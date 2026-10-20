import 'dart:async';

import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/render/render_port.dart';
import 'package:clax_app/ui/inspector_surface.dart' show InspectorLost;
import 'package:clax_app/ui/render_service.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_clock.dart' show settle;
import '../support/fake_inspector.dart';
import 'core_answers.dart';

/// Rendering asked of the UI isolate (spec 2026-10-19-in-process-core-design
/// §7.3): the worker's `Render` effects cross a [RenderPort] as
/// [RenderRequest]s, a [RenderService] answers each through its inspector,
/// and the answers come back as the queue's `Rendered`, `RenderLost` and
/// `CanRender` messages.
void main() {
  const origins = AppOrigins('clax://app', 'clax://{id}.c');

  group('the messages', () {
    test('each crosses the wire as a map and comes back equal', () {
      final png = Uint8List.fromList([1, 2, 3]);
      final all = <RenderMessage>[
        const RenderRequest(
          id: 7,
          jobId: 'j',
          callId: 'r1',
          draftId: 'd0000000000a',
          revision: 3,
          kind: RenderKind.tests,
          settleMs: 1500,
          maxBytes: 4096,
          files: ['tests/a.test.js'],
        ),
        const RenderRequest(
          id: 8,
          jobId: 'j',
          callId: 'r2',
          draftId: 'd0000000000a',
          revision: 3,
          kind: RenderKind.migration,
          migration: MigrationSpec(file: 'migrate.js', from: 1, to: 2),
        ),
        const RenderCancel('j'),
        RenderResult.done(
          7,
          'j',
          const RenderOutcome(clean: false, data: {'error': 'boom'}),
          png: png,
        ),
        const RenderResult.lost(8, 'j'),
        const RenderNews(canRender: false),
      ];
      for (final m in all) {
        final wire = encodeRender(m);
        expect(isRenderWire(wire), isTrue);
        expect(decodeRender(wire), m, reason: '$m');
      }
      expect(decodeRender({'kind': 'keepAlive'}), isNull);
      expect(isRenderWire('hello'), isFalse);
    });

    test('a request is the Render effect it was made of', () {
      const e = Render(
        4,
        'j',
        1,
        'r1',
        RenderSpec(draftId: 'd0000000000a', revision: 2),
        last: true,
      );
      final r = RenderRequest.of(e);
      expect(r.id, 4);
      expect(r.jobId, 'j');
      expect(r.callId, 'r1');
      expect(r.draftId, 'd0000000000a');
      expect(r.revision, 2);
      expect(r.kind, RenderKind.inspect);
    });

    test('replies are the queue\'s messages', () {
      const outcome = RenderOutcome(clean: true, data: {'inspection': {}});
      expect(
        RenderResult.done(4, 'j', outcome).toMsg(),
        isA<Rendered>()
            .having((m) => m.effect, 'effect', 4)
            .having((m) => m.jobId, 'jobId', 'j')
            .having((m) => m.outcome, 'outcome', outcome),
      );
      expect(
        const RenderResult.lost(4, 'j').toMsg(),
        isA<RenderLost>()
            .having((m) => m.effect, 'effect', 4)
            .having((m) => m.jobId, 'jobId', 'j'),
      );
      expect(
        const RenderNews(canRender: false).toMsg(),
        isA<CanRender>().having((m) => m.can, 'can', false),
      );
    });
  });

  group('the service', () {
    late FakeInspector inspector;
    late ValueNotifier<bool> available;
    late RenderService service;

    setUp(() {
      inspector = FakeInspector();
      available = ValueNotifier(true);
      service = RenderService(
        inspector: inspector,
        origins: origins,
        available: available,
      );
    });
    tearDown(() => service.dispose());

    RenderRequest req(
      int id, {
      String job = 'j',
      String call = 'r1',
      RenderKind kind = RenderKind.inspect,
      List<String> files = const [],
      MigrationSpec? migration,
    }) => RenderRequest(
      id: id,
      jobId: job,
      callId: call,
      draftId: 'd0000000000a',
      revision: 1,
      kind: kind,
      settleMs: 1500,
      maxBytes: 4096,
      files: files,
      migration: migration,
    );

    test('an inspection loads the draft in the shell\'s inspect view by '
        'the app\'s origins and reports what it shows', () async {
      inspector.answer(
        inspector.clean(
          errors: const [(kind: 'console', text: 'just a log')],
          failed: const ['clax://d0000000000a.c/missing.png'],
        ),
      );
      final r = await service.render(req(1));
      final call = inspector.calls.single;
      expect(call.url, Uri.parse('clax://app/a/d0000000000a?app=1&inspect=1'));
      expect(call.settle, const Duration(milliseconds: 1500));
      expect(call.maxBytes, 4096);
      expect(r.id, 1);
      expect(r.jobId, 'j');
      expect(r.png, inspector.png);
      expect(
        r.outcome,
        const RenderOutcome(
          clean: true,
          data: {
            'inspection': {
              'console_errors': [
                {'kind': 'console', 'text': 'just a log'},
              ],
              'failed_requests': ['clax://d0000000000a.c/missing.png'],
              'text': 'Runs\n[button] Add',
              'title': 'Runs',
              'height': 844,
              'settled_ms': 320,
            },
          },
        ),
      );
    });

    test('an uncaught error is not clean', () async {
      inspector.answer(
        inspector.clean(errors: const [(kind: 'error', text: 'x')]),
      );
      final r = await service.render(req(1));
      expect(r.outcome!.clean, isFalse);
    });

    test('a page that does not load is a render that failed, not one '
        'lost', () async {
      inspector.fail(const InspectorError('it did not load within 10 seconds'));
      final r = await service.render(req(1));
      expect(
        r.outcome,
        const RenderOutcome(
          clean: false,
          data: {'error': 'it did not load within 10 seconds'},
        ),
      );
    });

    test('a surface torn down under the render is a render lost', () async {
      inspector.fail(const InspectorLost('the app is closing'));
      final r = await service.render(req(1));
      expect(r.isLost, isTrue);
    });

    test('tests and migrations run by the inspector', () async {
      inspector.answerTests(
        const TestRun(
          passed: 1,
          failed: [(name: 'adds', error: 'no')],
          skipped: 0,
        ),
      );
      final t = await service.render(
        req(1, kind: RenderKind.tests, files: const ['tests/a.test.js']),
      );
      expect(inspector.testCalls.single.files, ['tests/a.test.js']);
      expect(
        t.outcome,
        const RenderOutcome(
          clean: false,
          data: {
            'tests': {
              'passed': 1,
              'failed': [
                {'name': 'adds', 'error': 'no'},
              ],
              'skipped': 0,
            },
          },
        ),
      );

      final m = await service.render(
        req(
          2,
          kind: RenderKind.migration,
          migration: const MigrationSpec(file: 'migrate.js', from: 1, to: 2),
        ),
      );
      final mc = inspector.migrationCalls.single;
      expect((mc.file, mc.from, mc.to), ('migrate.js', 1, 2));
      expect(
        m.outcome,
        const RenderOutcome(
          clean: true,
          data: {
            'migration': {'ok': true},
          },
        ),
      );

      final none = await service.render(req(3, kind: RenderKind.tests));
      expect(none.outcome!.clean, isFalse);
      expect(inspector.testCalls, hasLength(1), reason: 'no files, no run');
    });

    test('a render asked while none can be made is lost at once', () async {
      available.value = false;
      expect(service.canRender.value, isFalse);
      final r = await service.render(req(1));
      expect(r.isLost, isTrue);
      expect(inspector.calls, isEmpty);
    });

    test('the app going away mid-render loses it and stops the page', () async {
      inspector.hang();
      final f = service.render(req(1));
      await settle();
      available.value = false;
      final r = await f;
      expect(r.isLost, isTrue);
      await settle();
      expect(inspector.calls.single.cancelled, isNotNull);
    });

    test('a request again for the same call ends the one before; a job\'s '
        'cancel ends its renders', () async {
      inspector
        ..hang()
        ..hang();
      final first = service.render(req(1));
      await settle();
      final second = service.render(req(2));
      expect((await first).isLost, isTrue);
      service.cancel('j');
      expect((await second).isLost, isTrue);
    });

    test('serving an endpoint says whether it can render, then on each '
        'change, and answers each request', () async {
      final (port, end) = inProcessRender();
      final replies = <RenderReply>[];
      final sub = port.replies.listen(replies.add);
      final serving = service.serve(end);
      await settle();
      expect(replies, [const RenderNews(canRender: true)]);

      port.ask(req(5));
      await settle();
      expect(replies.last, isA<RenderResult>().having((r) => r.id, 'id', 5));

      available.value = false;
      available.value = false;
      await settle();
      expect(replies.whereType<RenderNews>().toList(), const [
        RenderNews(canRender: true),
        RenderNews(canRender: false),
      ]);

      inspector.hang();
      available.value = true;
      port.ask(req(6));
      await settle();
      port.cancel('j');
      await settle();
      expect(
        replies.last,
        isA<RenderResult>()
            .having((r) => r.id, 'id', 6)
            .having((r) => r.isLost, 'lost', isTrue),
      );
      serving.close();
      await sub.cancel();
    });
  });

  group('the worker and the UI isolate', () {
    late FakeInspector inspector;
    late ValueNotifier<bool> available;
    late RenderService service;
    late _RenderWorld w;

    setUp(() {
      inspector = FakeInspector();
      available = ValueNotifier(true);
      service = RenderService(
        inspector: inspector,
        origins: origins,
        available: available,
      );
      final (port, end) = inProcessRender();
      service.serve(end);
      w = _RenderWorld(FakeJobsCore(), port);
    });
    tearDown(() async {
      await w.close();
      service.dispose();
    });

    test('a render answered by the inspector reaches the worker as '
        'Rendered, and the job goes on', () async {
      w.start();
      w.send(const Make('j', 'a todo app'));
      await w.drive();
      expect(inspector.calls, hasLength(1));
      final rendered = w.received.whereType<Rendered>().single;
      expect(rendered.outcome.clean, isTrue);
      expect(w.core.read('j')!.state, JobState.done);
    });

    test('a render lost to a reload is asked again once, then waits', () async {
      inspector.hang();
      w.start();
      w.send(const Make('j', 'a todo app'));
      await w.drive(until: () => inspector.calls.isNotEmpty);
      expect(w.effects.whereType<Render>(), hasLength(1));
      final turns = w.effects.whereType<StartTurn>().length;
      final row = w.core.read('j')!;

      // The UI engine detaches mid-render: the WebView is torn down.
      available.value = false;
      await w.drive();
      final renders = w.effects.whereType<Render>().toList();
      expect(renders, hasLength(2), reason: 'asked again once');
      expect(renders.last.last, isTrue);
      expect(w.received.whereType<RenderLost>(), hasLength(2));
      expect(
        viewOf(w.m).jobs.single.waitingFor,
        WaitingFor.render,
        reason: 'the second lost too: the job waits',
      );
      expect(w.core.read('j'), row, reason: 'its core row unchanged');
      expect(w.effects.whereType<StartTurn>(), hasLength(turns));

      // The engine is back: one more render, and the job goes on.
      available.value = true;
      await w.drive(until: () => w.effects.whereType<Render>().length > 2);
      expect(w.effects.whereType<Render>(), hasLength(3));
      expect(
        w.effects.whereType<StartTurn>(),
        hasLength(turns),
        reason: 'no turn while it waited',
      );
      await w.drive();
      expect(w.received.whereType<Rendered>(), hasLength(1));
      expect(w.core.read('j')!.state, JobState.done);
    });
  });
}

/// A worker whose `Render` effects go to the UI isolate through [port], as
/// the effect runner sends them, and whose replies come back as messages.
class _RenderWorld extends World {
  _RenderWorld(super.core, this.port) {
    _sub = port.replies.listen((r) {
      final msg = r.toMsg();
      received.add(msg);
      inbox.add(msg);
    });
  }

  final RenderPort port;
  late final StreamSubscription<RenderReply> _sub;

  /// Every message the UI isolate's replies became.
  final List<Msg> received = [];

  @override
  Msg? answerOf(Effect e) {
    if (e is Render) {
      port.ask(RenderRequest.of(e));
      return null;
    }
    if (e is StopWork) port.cancel(e.jobId);
    return super.answerOf(e);
  }

  /// Runs the worker and lets the UI isolate answer, until neither has
  /// anything left to do or [until] holds.
  Future<void> drive({bool Function()? until}) async {
    for (var i = 0; i < 100; i++) {
      run(until: until);
      if (until != null && until()) return;
      await settle();
      if (inbox.isEmpty && core.events.isEmpty) return;
    }
    throw StateError('the worker and the UI isolate did not settle');
  }

  Future<void> close() => _sub.cancel();
}
