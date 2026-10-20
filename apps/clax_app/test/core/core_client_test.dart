import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';
import '../support/paths.dart';

/// The core's response in [a]; fails the test when the call went
/// unanswered.
CoreResponse answered(CoreAnswer a) => switch (a) {
  CoreAnswered(:final response) => response,
  CoreUnanswered() => throw TestFailure('expected an answer, got $a'),
};

/// The code of [a] when the call went unanswered; null when answered.
String? unanswered(CoreAnswer a) => switch (a) {
  CoreAnswered() => null,
  CoreUnanswered(:final code) => code,
};

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
      expect(unanswered(r), 'core_failed');
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
      final answers = (await Future.wait([a, b, c])).map(answered);
      expect(
        [for (final r in answers) (r.status, utf8.decode(r.body))],
        [(201, 'a'), (404, 'b'), (200, 'c')],
      );
    });

    test('an unknown handle goes unanswered as core_closed', () async {
      final core = open();
      lib.callAnswer = 1;
      final r = await core.call(const CoreRequest('GET', '/api/a'));
      expect(unanswered(r), 'core_closed');
    });

    test('a frame the core refuses goes unanswered as frame_refused', () async {
      final core = open();
      lib.callAnswer = 2;
      final r = await core.call(const CoreRequest('GET', '/api/a'));
      expect(unanswered(r), 'frame_refused');
    });

    test("the core's own 503 core_closed and core_failed are unanswered", () async {
      final core = open();
      final closed = core.call(const CoreRequest('GET', '/api/a'));
      final failed = core.call(const CoreRequest('GET', '/api/b'));
      final busy = core.call(const CoreRequest('GET', '/api/c'));
      String error(String code) =>
          jsonEncode({'error': {'code': code, 'message': 'from the core'}});
      const json = [('content-type', 'application/json')];
      lib.answer(lib.calls[0], 503, headers: json, body: error('core_closed'));
      lib.answer(lib.calls[1], 503, headers: json, body: error('core_failed'));
      lib.answer(lib.calls[2], 503, headers: json, body: error('busy'));
      final c = await closed;
      expect(c, isA<CoreUnanswered>());
      expect((c as CoreUnanswered).code, 'core_closed');
      expect(c.message, 'from the core');
      expect(unanswered(await failed), 'core_failed');
      // A route's own 503 is an answer.
      expect(answered(await busy).status, 503);
    });

    test('an answer that is not a frame goes unanswered as malformed', () async {
      final core = open();
      final r = core.call(const CoreRequest('GET', '/api/a'));
      final c = lib.calls.single;
      c.port.send(Uint8List.fromList([
        ...(ByteData(8)..setInt64(0, c.tag, Endian.little)).buffer.asUint8List(),
        ...ascii.encode('CLX2xxxxxxxx'),
      ]));
      expect(unanswered(await r), 'malformed');
    });

    test('a failed open ends every call, sent or not, as core_failed', () async {
      final core = open();
      final pending = core.call(const CoreRequest('GET', '/api/a'));
      lib.postState({
        'type': 'failed',
        'code': 'open_failed',
        'message': 'no store',
      });
      expect(unanswered(await pending), 'core_failed');
      final after = await core.call(const CoreRequest('GET', '/api/b'));
      expect(unanswered(after), 'core_failed');
      expect(lib.calls, hasLength(1), reason: 'a failed core is not called');
      // The core's own late answer to the first call is dropped.
      lib.answer(lib.calls.single, 503, body: 'late');
      await pumpEventQueue();
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
      expect(unanswered(late), 'core_closed');
      expect(lib.calls, hasLength(2));

      // A call in flight still gets the core's answer while it drains.
      lib.answer(lib.calls[0], 200, body: 'ok');
      expect(answered(await drained).status, 200);

      // The core's `closed` ends every call it did not answer.
      lib.postState({'type': 'closed'});
      expect(unanswered(await cut), 'core_closed');
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
      expect(unanswered(await pending), 'core_closed');
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

    test('a resume point is ignored until the C functions carry one', () async {
      // The core opens a new stream, whose ready says resumed: false, and
      // SseClaxStream resyncs its topics for it.
      final core = open();
      final sub = core.subscribe(lastEventId: 's1:42');
      final s = lib.subscriptions.single;
      lib.postBlock(s, 'event: ready\ndata: {"stream":"s2","resumed":false}\n\n');
      lib.postBlock(s, endBlock('stream_ended'));
      expect(await sub.blocks.first, contains('"resumed":false'));
    });

    test('the core closing ends a subscription it left open', () async {
      final core = open();
      final sub = core.subscribe();
      core.close();
      lib.postState({'type': 'closed'});
      expect(await sub.blocks.toList(), [endBlock('core_closed')]);
    });
  });

  group('malformed input', () {
    /// Posts [c]'s tag, then a frame whose head is [head].
    void postHead(FakeLibCall c, String head) {
      final h = utf8.encode(head);
      c.port.send(Uint8List.fromList([
        ...(ByteData(8)..setInt64(0, c.tag, Endian.little)).buffer.asUint8List(),
        ...ascii.encode('CLX1'),
        ...(ByteData(4)..setUint32(0, h.length, Endian.little)).buffer.asUint8List(),
        ...h,
      ]));
    }

    test('a status outside 100-599, or not an integer, is malformed', () async {
      final core = open();
      for (final status in ['0', '99', '600', '1e400', '200.5', '"200"', 'null']) {
        final r = core.call(const CoreRequest('GET', '/api/a'));
        postHead(lib.calls.last, '{"status":$status,"headers":[]}');
        expect(unanswered(await r), 'malformed', reason: status);
      }
    });

    test('a message too short for its tag is logged; its call waits for close', () async {
      final logs = <String>[];
      final core = CoreClient.open(
        home.path,
        AppOrigins.forPlatform(android: true),
        lib: lib,
        log: logs.add,
      );
      final r = core.call(const CoreRequest('GET', '/api/a'));
      lib.calls.single.port.send(Uint8List(3));
      lib.calls.single.port.send('not bytes');
      await pumpEventQueue();
      expect(logs, hasLength(2));
      core.close();
      lib.postState({'type': 'closed'});
      expect(unanswered(await r), 'core_closed');
    });

    test('a frame too short for its head, after a tag, is malformed', () async {
      final core = open();
      final r = core.call(const CoreRequest('GET', '/api/a'));
      final c = lib.calls.single;
      c.port.send(Uint8List.fromList([
        ...(ByteData(8)..setInt64(0, c.tag, Endian.little)).buffer.asUint8List(),
        ...ascii.encode('CLX'),
      ]));
      expect(unanswered(await r), 'malformed');
    });

    test('malformed UTF-8 ends a subscription as malformed, and is logged', () async {
      final logs = <String>[];
      final core = CoreClient.open(
        home.path,
        AppOrigins.forPlatform(android: true),
        lib: lib,
        log: logs.add,
      );
      final sub = core.subscribe();
      final s = lib.subscriptions.single;
      lib.postBlock(s, 'event: ready\n\n');
      lib.postBytes(s, [...ascii.encode('data: '), 0xff, 0x0a, 0x0a]);
      lib.postBlock(s, 'event: after\n\n');
      expect(await sub.blocks.toList(), ['event: ready\n\n', endBlock('malformed')]);
      expect(logs, hasLength(1));
    });

    test('an end block split across posts ends with its reason', () async {
      final core = open();
      final sub = core.subscribe();
      final s = lib.subscriptions.single;
      final end = endBlock('stream_ended');
      lib.postBlock(s, end.substring(0, 'event: end\n'.length));
      lib.postBlock(s, end.substring('event: end\n'.length, end.length - 1));
      lib.postBlock(s, '\n');
      lib.postBlock(s, 'event: after\n\n');
      expect(await sub.blocks.toList(), [end]);
    });

    test('blocks are yielded whole, however the posts split them', () async {
      final core = open();
      final sub = core.subscribe();
      final s = lib.subscriptions.single;
      final bytes = utf8.encode('data: "café ✓"\n\nevent: x\n\n');
      lib.postBytes(s, bytes.sublist(0, 11));
      lib.postBytes(s, bytes.sublist(11, 18));
      lib.postBytes(s, bytes.sublist(18));
      lib.postBlock(s, endBlock('stream_ended'));
      expect(await sub.blocks.toList(), [
        'data: "café ✓"\n\n',
        'event: x\n\n',
        endBlock('stream_ended'),
      ]);
    });
  });
}
