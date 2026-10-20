import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';
import '../support/paths.dart';

/// The JSON error code of [r]'s body.
String? errorCode(CoreResponse r) =>
    ((jsonDecode(utf8.decode(r.body)) as Map)['error'] as Map?)?['code']
        as String?;

void main() {
  late Directory home;
  late FakeCoreLib lib;

  setUp(() {
    home = scratchRoot().createTempSync('core-client-');
    lib = FakeCoreLib();
  });
  tearDown(() => home.deleteSync(recursive: true));

  CoreClient open() => CoreClient.open(
    home.path,
    AppOrigins.forPlatform(android: true),
    lib: lib,
  );

  group('open', () {
    test('writes embed.json before it opens, and reports each phase', () async {
      final core = open();
      expect(lib.opens.single.home, home.path);
      expect(jsonDecode(lib.opens.single.embedJson!), {
        'shell': 'https://app.clax.invalid',
        'content': 'https://{id}.c.clax.invalid',
      });
      expect(core.handle, lib.openAnswer);

      lib.postState({'type': 'opening', 'phase': 'backing up'});
      lib.postState({
        'type': 'opening',
        'phase': 'migrating',
        'done': 0,
        'total': 2,
      });
      lib.postState({
        'type': 'opening',
        'phase': 'migrating',
        'done': 1,
        'total': 2,
      });
      lib.postState({'type': 'ready'});
      lib.postState({'type': 'closed'});
      await expectLater(
        core.states,
        emitsInOrder([
          const CoreOpening('backing up'),
          const CoreOpening('migrating', done: 0, total: 2),
          const CoreOpening('migrating', done: 1, total: 2),
          const CoreReady(),
          const CoreClosed(),
          emitsDone,
        ]),
      );
    });

    test('a failed open names its code, message and backup', () async {
      final core = open();
      lib.postState({
        'type': 'failed',
        'code': 'migration_failed',
        'message': 'migration 44 failed',
        'backup': '/h/backups/clax.db.1',
      });
      expect(
        await core.states.first,
        const CoreFailed(
          'migration_failed',
          'migration 44 failed',
          '/h/backups/clax.db.1',
        ),
      );
    });

    test('a home the core refuses (handle 0) fails as home_in_use', () async {
      lib.openAnswer = 0;
      final core = open();
      final states = <CoreOpen>[];
      final done = Completer<void>();
      core.states.listen(states.add, onDone: done.complete);
      final r = await core.call(const CoreRequest('GET', '/api/artifacts'));
      expect((r.status, errorCode(r)), (0, 'core_failed'));
      expect(lib.calls, isEmpty);
      final sub = core.subscribe();
      expect(await sub.blocks.toList(), [endBlock('core_failed')]);
      core.close();
      await done.future;
      expect(states, [isA<CoreFailed>(), const CoreClosed()]);
      expect((states.first as CoreFailed).code, 'home_in_use');
      expect(lib.closes, isEmpty);
    });
  });

  group('call', () {
    test('answers concurrent calls by tag, in any order', () async {
      final core = open();
      final a = core.call(const CoreRequest('GET', '/api/a'));
      final b = core.call(
        CoreRequest('POST', '/api/b', body: utf8.encode('{"x":1}')),
      );
      final c = core.call(const CoreRequest('GET', '/api/c'));
      expect(lib.calls.map((c) => c.request.path), ['/api/a', '/api/b', '/api/c']);
      expect(lib.calls.map((c) => c.core).toSet(), {core.handle});
      expect(lib.calls.map((c) => c.tag).toSet(), hasLength(3));
      expect(utf8.decode(lib.calls[1].request.body!), '{"x":1}');

      lib.answer(lib.calls[2], 200, body: 'c');
      lib.answer(lib.calls[0], 201, body: 'a');
      lib.answer(lib.calls[1], 404, body: 'b');
      final answers = await Future.wait([a, b, c]);
      expect(
        [for (final r in answers) (r.status, utf8.decode(r.body))],
        [(201, 'a'), (404, 'b'), (200, 'c')],
      );
    });

    test('an unknown handle answers status 0 core_closed', () async {
      final core = open();
      lib.callAnswer = 1;
      final r = await core.call(const CoreRequest('GET', '/api/a'));
      expect((r.status, errorCode(r)), (0, 'core_closed'));
    });

    test('a frame the core refuses is an ArgumentError', () async {
      final core = open();
      lib.callAnswer = 2;
      await expectLater(
        core.call(const CoreRequest('GET', '/api/a')),
        throwsArgumentError,
      );
    });
  });

  group('close', () {
    test('lets calls in flight drain, then answers the rest core_closed', () async {
      final core = open();
      final states = <CoreOpen>[];
      core.states.listen(states.add);
      lib.postState({'type': 'ready'});
      final drained = core.call(const CoreRequest('GET', '/api/drained'));
      final cut = core.call(const CoreRequest('GET', '/api/cut'));
      core.close();
      expect(lib.closes, [core.handle]);
      core.close();
      expect(lib.closes, [core.handle], reason: 'close is idempotent');

      // A call after close never reaches the core.
      final late = await core.call(const CoreRequest('GET', '/api/late'));
      expect((late.status, errorCode(late)), (0, 'core_closed'));
      expect(lib.calls, hasLength(2));

      // A call in flight still gets the core's answer while it drains.
      lib.answer(lib.calls[0], 200, body: 'ok');
      expect((await drained).status, 200);

      // The core's `closed` ends every call it did not answer.
      lib.postState({'type': 'closed'});
      final r = await cut;
      expect((r.status, errorCode(r)), (0, 'core_closed'));
      await pumpEventQueue();
      expect(states, [const CoreReady(), const CoreClosed()]);
      // A late answer to a call already ended is dropped.
      lib.answer(lib.calls[1], 503, body: 'late');
      await pumpEventQueue();
    });

    test('an attached client closes only its own door', () async {
      final core = CoreClient.attach(7, lib: lib);
      final pending = core.call(const CoreRequest('GET', '/api/a'));
      expect(lib.calls.single.core, 7);
      core.close();
      final r = await pending;
      expect((r.status, errorCode(r)), (0, 'core_closed'));
      expect(lib.closes, isEmpty);
      expect(await core.states.isEmpty, isTrue);
    });
  });

  group('subscribe', () {
    test('yields the blocks the core posts, through the end block', () async {
      for (final reason in [
        'core_failed',
        'core_closed',
        'stream_refused',
        'stream_ended',
      ]) {
        final core = open();
        final sub = core.subscribe();
        final s = lib.subscriptions.last;
        expect(s.core, core.handle);
        lib.postBlock(s, 'id: 1\nevent: ready\ndata: {"stream":"s1"}\n\n');
        lib.postBlock(s, ': keep-alive\n\n');
        lib.postBlock(s, endBlock(reason));
        lib.postBlock(s, 'event: after\n\n');
        expect(await sub.blocks.toList(), [
          'id: 1\nevent: ready\ndata: {"stream":"s1"}\n\n',
          ': keep-alive\n\n',
          endBlock(reason),
        ], reason: reason);
        core.close();
        lib.postState({'type': 'closed'}, open: lib.opens.length - 1);
      }
    });

    test('a character split across two posts decodes whole', () async {
      final core = open();
      final sub = core.subscribe();
      final s = lib.subscriptions.single;
      final bytes = utf8.encode('data: "café ✓"\n\n');
      lib.postBytes(s, bytes.sublist(0, 11));
      lib.postBytes(s, bytes.sublist(11));
      lib.postBlock(s, endBlock('stream_ended'));
      expect((await sub.blocks.toList()).join(), 'data: "café ✓"\n\n${endBlock('stream_ended')}');
    });

    test('cancel stops the blocks and closes the port', () async {
      final core = open();
      final sub = core.subscribe();
      final s = lib.subscriptions.single;
      final seen = <String>[];
      final done = Completer<void>();
      sub.blocks.listen(seen.add, onDone: done.complete);
      lib.postBlock(s, 'event: ready\n\n');
      await pumpEventQueue();
      sub.cancel();
      await done.future;
      lib.postBlock(s, 'event: version\n\n');
      await pumpEventQueue();
      expect(seen, ['event: ready\n\n']);
    });

    test('an unknown handle ends at once with core_closed', () async {
      final core = open();
      lib.subscribeAnswer = 1;
      expect(await core.subscribe().blocks.toList(), [endBlock('core_closed')]);
    });

    test('a resume point is refused until the C functions carry one', () {
      final core = open();
      expect(
        () => core.subscribe(lastEventId: '42'),
        throwsUnsupportedError,
      );
      expect(lib.subscriptions, isEmpty);
    });

    test('the core closing ends a subscription it left open', () async {
      final core = open();
      final sub = core.subscribe();
      core.close();
      lib.postState({'type': 'closed'});
      expect(await sub.blocks.toList(), [endBlock('core_closed')]);
    });
  });
}
