import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/queue/codec.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/render.dart';
import 'package:clax_app/ui/inspector_surface.dart' show InspectorLost;
import 'package:clax_app/ui/render_service.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_clock.dart' show settle;
import '../support/fake_inspector.dart';
import 'core_answers.dart';

/// Rendering asked of the UI isolate (spec 2026-10-19-in-process-core-design
/// §7.3): the worker's `Render` effects cross a [RenderPort] as the codec's
/// [UiRender]s, a [RenderService] answers each through its inspector, and
/// the answers come back as the queue's `Rendered`, `RenderLost` and
/// `CanRender` messages.
void main() {
  const origins = AppOrigins('clax://app', 'clax://{id}.c');

  group('the messages', () {
    test('a request is the Render effect it was made of', () {
      const spec = RenderSpec(
        draftId: 'd0000000000a',
        revision: 2,
        digest: 'sha256:ab',
        files: ['tests/a.test.js'],
      );
      const e = Render(4, 'j', 1, 'r1', spec, last: true);
      final r = RenderRequest.of(e);
      expect(r.effect, 4);
      expect(r.jobId, 'j');
      expect(r.callId, 'r1');
      expect(r.spec, spec);
    });

    test('answers are the queue\'s messages', () {
      const outcome = RenderOutcome(clean: true, data: {'inspection': {}});
      expect(
        const RenderAnswered(4, 'j', outcome).toMsg(),
        isA<Rendered>()
            .having((m) => m.effect, 'effect', 4)
            .having((m) => m.jobId, 'jobId', 'j')
            .having((m) => m.outcome, 'outcome', outcome),
      );
      expect(
        const RenderGone(4, 'j').toMsg(),
        isA<RenderLost>()
            .having((m) => m.effect, 'effect', 4)
            .having((m) => m.jobId, 'jobId', 'j'),
      );
    });

    test('an outcome names what broke, and one not clean never reads as '
        'clean', () {
      expect(
        const RenderOutcome(
          clean: false,
          data: {
            'errors': ['x is undefined'],
            'failed_tests': ['adds'],
          },
        ).errors,
        ['x is undefined'],
      );
      expect(
        const RenderOutcome(clean: false, data: {'error': 'did not load'})
            .errors,
        ['did not load'],
      );
      expect(const RenderOutcome(clean: false).errors, isNotEmpty);
      expect(
        const RenderOutcome(
          clean: false,
          data: {
            'failed_tests': ['adds'],
          },
        ).errors,
        isEmpty,
        reason: 'a failing test is what broke',
      );
      expect(const RenderOutcome(clean: true).errors, isEmpty);
    });

    test('the in-process wire carries each message through the codec, and '
        'a port answers only what it asked', () async {
      final loop = <Msg>[];
      final (port, end) = inProcessRender(toLoop: loop.add);
      final asks = <UiRenderAsk>[];
      end.asks.listen(asks.add);
      const spec = RenderSpec(draftId: 'd', revision: 1);
      final answered = port.render(const RenderRequest(1, 'j', 'r', spec));
      await settle();
      expect(asks, [const UiRender(RenderRequest(1, 'j', 'r', spec))]);
      end.answer(const RenderAnswered(9, 'j', RenderOutcome(clean: true)));
      end.answer(const RenderAnswered(1, 'j', RenderOutcome(clean: true)));
      end.canRender(false);
      expect(
        await answered,
        const RenderAnswered(1, 'j', RenderOutcome(clean: true)),
      );
      await settle();
      expect(loop.single, isA<CanRender>().having((m) => m.can, 'can', false));

      final cancelled = port.render(const RenderRequest(2, 'j', 'r', spec));
      port.cancel('j');
      expect(await cancelled, const RenderGone(2, 'j'));
      await settle();
      expect(asks.last, const UiRenderCancel('j'));

      final pending = port.render(const RenderRequest(3, 'k', 'r', spec));
      port.close();
      expect(await pending, const RenderGone(3, 'k'));
      expect(
        await port.render(const RenderRequest(4, 'k', 'r', spec)),
        const RenderGone(4, 'k'),
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
      id,
      job,
      call,
      RenderSpec(
        draftId: 'd0000000000a',
        revision: 1,
        kind: kind,
        files: files,
        migration: migration,
      ),
      settleMs: 1500,
      maxBytes: 4096,
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
      expect(
        r,
        RenderAnswered(
          1,
          'j',
          RenderOutcome(
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
                'png': base64Encode(inspector.png),
              },
              'errors': const <String>[],
              'failed_tests': const <String>[],
            },
          ),
        ),
      );
    });

    test('an uncaught error is not clean, and is named', () async {
      inspector.answer(
        inspector.clean(
          errors: const [
            (kind: 'error', text: 'x'),
            (kind: 'console', text: 'a log'),
          ],
        ),
      );
      final r = await service.render(req(1)) as RenderAnswered;
      expect(r.outcome.clean, isFalse);
      expect(r.outcome.errors, ['x']);
    });

    test('a page that does not load is a render that failed, not one '
        'lost', () async {
      inspector.fail(const InspectorError('it did not load within 10 seconds'));
      final r = await service.render(req(1));
      expect(
        r,
        const RenderAnswered(
          1,
          'j',
          RenderOutcome(
            clean: false,
            data: {
              'error': 'it did not load within 10 seconds',
              'errors': ['it did not load within 10 seconds'],
              'failed_tests': <String>[],
            },
          ),
        ),
      );
    });

    test('a surface torn down under the render is a render lost', () async {
      inspector.fail(const InspectorLost('the app is closing'));
      expect(await service.render(req(1)), const RenderGone(1, 'j'));
    });

    test('tests and migrations run by the inspector, from the spec\'s files '
        'and migration', () async {
      inspector.answerTests(
        const TestRun(
          passed: 1,
          failed: [(name: 'adds', error: 'no')],
          skipped: 0,
        ),
      );
      final t = await service.render(
        req(1, kind: RenderKind.tests, files: const ['tests/a.test.js']),
      ) as RenderAnswered;
      expect(inspector.testCalls.single.files, ['tests/a.test.js']);
      expect(t.outcome.clean, isFalse);
      expect(t.outcome.failedTests, ['adds']);
      expect((t.outcome.data! as Map)['tests'], {
        'passed': 1,
        'failed': [
          {'name': 'adds', 'error': 'no'},
        ],
        'skipped': 0,
      });

      final m = await service.render(
        req(
          2,
          kind: RenderKind.migration,
          migration: const MigrationSpec(file: 'migrate.js', from: 1, to: 2),
        ),
      ) as RenderAnswered;
      final mc = inspector.migrationCalls.single;
      expect((mc.file, mc.from, mc.to), ('migrate.js', 1, 2));
      expect(m.outcome.clean, isTrue);
      expect((m.outcome.data! as Map)['migration'], {'ok': true});

      final none = await service.render(req(3, kind: RenderKind.tests))
          as RenderAnswered;
      expect(none.outcome.clean, isFalse);
      expect(inspector.testCalls, hasLength(1), reason: 'no files, no run');
    });

    test('a render asked while none can be made is lost at once', () async {
      available.value = false;
      expect(service.canRender.value, isFalse);
      expect(await service.render(req(1)), const RenderGone(1, 'j'));
      expect(inspector.calls, isEmpty);
    });

    test('the app going away mid-render loses it and stops the page', () async {
      inspector.hang();
      final f = service.render(req(1));
      await settle();
      available.value = false;
      expect(await f, const RenderGone(1, 'j'));
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
      expect(await first, const RenderGone(1, 'j'));
      service.cancel('j');
      expect(await second, const RenderGone(2, 'j'));
    });

    test('serving an endpoint says whether it can render, then on each '
        'change, and answers each request', () async {
      final news = <bool>[];
      final (port, end) = inProcessRender(
        toLoop: (m) => news.add((m as CanRender).can),
      );
      final serving = service.serve(end);
      await settle();
      expect(news, [true]);

      final r5 = await port.render(req(5));
      expect(r5, isA<RenderAnswered>().having((r) => r.effect, 'effect', 5));

      available.value = false;
      available.value = false;
      await settle();
      expect(news, [true, false]);

      inspector.hang();
      available.value = true;
      final r6 = port.render(req(6));
      await settle();
      port.cancel('j');
      expect(await r6, const RenderGone(6, 'j'));
      await settle();
      expect(inspector.calls.last.cancelled, isNotNull);
      serving.close();
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
      w = _RenderWorld(FakeJobsCore());
      service.serve(w.end);
    });
    tearDown(() => service.dispose());

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

/// A worker whose `Render` effects go to the UI isolate through an
/// in-process [RenderPort], as the effect runner sends them, and whose
/// answers and news come back as messages.
class _RenderWorld extends World {
  _RenderWorld(super.core) {
    final (port, end) = inProcessRender(
      toLoop: (m) {
        received.add(m);
        inbox.add(m);
      },
    );
    this.port = port;
    this.end = end;
  }

  late final WireRenderPort port;
  late final WireRenderEndpoint end;

  /// Every message the UI isolate's answers and news became.
  final List<Msg> received = [];

  @override
  Msg? answerOf(Effect e) {
    if (e is Render) {
      unawaited(
        port.render(RenderRequest.of(e)).then((r) {
          final m = r.toMsg();
          received.add(m);
          inbox.add(m);
        }),
      );
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
}
