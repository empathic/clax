import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/messages_runner.dart';
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/tool_spec.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/platform/network_pause.dart';
import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';

import '../agent/tools/support.dart';

// Pausing the runs when iOS's background time is nearly spent
// (AppPresence.pause), and resuming them when Clax is back.

const _dns = AgentError(
  "Couldn't reach the Claude API: SocketException: Failed host lookup",
  retryable: true,
  kind: AgentErrorKind.network,
);

ModelTurn _turn(List<Map<String, Object?>> content) => ModelTurn(
  content: content,
  stopReason: content.any((b) => b['type'] == 'tool_use')
      ? 'tool_use'
      : 'end_turn',
  usage: const Usage(input: 100, output: 10),
);

final _ok = _turn([
  {'type': 'text', 'text': 'Done.'},
]);

Map<String, Object?> _use(String id, String name) => {
  'type': 'tool_use',
  'id': id,
  'name': name,
  'input': <String, Object?>{},
};

/// A turn that streams an event, runs [during], then streams until it is
/// cancelled, as a request in flight does; [aborted] is set once its
/// `cancelled` fires.
class _Live {
  _Live([this.during]);
  final void Function()? during;
  var aborted = false;
}

/// Answers each turn with the next item: a [ModelTurn] is returned, a
/// [_Live] streams until cancelled, a callback runs and then [_dns] is
/// thrown (a network cut).
class _Turns implements TurnBackend {
  _Turns(this.script);
  final List<Object> script;
  final seen = <TurnRequest>[];

  @override
  String get name => 'fake';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    seen.add(r);
    final item = script[seen.length - 1];
    if (item is _Live) {
      r.onActivity?.call();
      item.during?.call();
      await r.cancelled;
      item.aborted = true;
      throw const AgentError.cancelled();
    }
    if (item is void Function()) {
      item();
      throw _dns;
    }
    return item as ModelTurn;
  }
}

TurnRequest _turnRequest({Future<void>? cancelled}) => TurnRequest(
  model: 'm',
  system: 's',
  tools: const [],
  messages: const [
    {
      'role': 'user',
      'content': [
        {'type': 'text', 'text': 'u'},
      ],
    },
  ],
  outputCap: 20000,
  cancelled: cancelled,
);

const _result = EditResult(
  reply: 'ok',
  title: 'T',
  files: [],
  edits: [],
  removed: [],
  db: [],
  capabilities: {},
  resolve: false,
);

/// A single call that runs until it is cancelled ([aborted]), then, once
/// [live] is false, answers [_result].
class _Call implements AgentBackend {
  var runs = 0;
  var aborted = 0;
  var live = true;

  @override
  String get name => 'fake';

  @override
  Future<EditResult> run(AgentRequest request) async {
    runs++;
    if (!live) return _result;
    await request.cancelled;
    aborted++;
    throw const AgentError.cancelled();
  }
}

/// A tool that counts its calls; [ends] finishes the run.
class _Counted implements Tool {
  _Counted(this.name, {this.ends = false});
  final String name;
  final bool ends;
  var calls = 0;

  @override
  ToolSpec get spec => ToolSpec(name, name, const {
    'type': 'object',
    'properties': <String, Object?>{},
    'required': <String>[],
    'additionalProperties': false,
  });

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    calls++;
    if (ends) {
      s.outcome = const RunFinished(
        artifactId: 'a1',
        version: 4,
        answered: true,
      );
    }
    return const ToolOk({});
  }
}

void main() {
  late AppPresence p;
  late StepClock clock;

  setUp(() {
    p = AppPresence();
    clock = StepClock();
  });

  void leave() => p.onLifecycle(AppLifecycleState.paused);
  void back() => p.onLifecycle(AppLifecycleState.resumed);

  group('AppPresence.pause', () {
    test('pauses only out of the foreground, until the app is back', () async {
      var fired = false;
      unawaited(p.whenPaused.then((_) => fired = true));
      p.pause();
      await pumpEventQueue();
      expect((p.paused, fired), (false, false), reason: 'in front');

      leave();
      p.pause();
      await pumpEventQueue();
      expect((p.paused, fired), (true, true));
      var again = false;
      unawaited(p.whenPaused.then((_) => again = true));
      await pumpEventQueue();
      expect(again, isTrue, reason: 'at once while paused');

      back();
      expect(p.paused, isFalse);
      var next = false;
      unawaited(p.whenPaused.then((_) => next = true));
      leave();
      await pumpEventQueue();
      expect((p.paused, next), (false, false), reason: 'a leave is no pause');
      p.pause();
      await pumpEventQueue();
      expect(next, isTrue);
    });
  });

  group('ResumingTurns', () {
    test('a turn in flight is aborted by the pause, held however long the '
        'app is away, and sent again unchanged on return', () async {
      final live = _Live();
      final inner = _Turns([live, _ok]);
      final events = <String>[];
      final turns = ResumingTurns(
        inner,
        p,
        clock: clock,
        onPause: () => events.add('pause'),
        onResume: () => events.add('resume'),
      );
      final r = _turnRequest();
      ModelTurn? got;
      unawaited(turns.turn(r).then((t) => got = t));
      await pumpEventQueue();
      leave();
      await pumpEventQueue();
      expect(live.aborted, isFalse, reason: 'leaving alone does not abort');

      p.pause();
      await pumpEventQueue();
      expect(live.aborted, isTrue, reason: 'aborted at once');
      expect(events, ['pause']);

      clock.advance(const Duration(hours: 5));
      await pumpEventQueue();
      expect((got, inner.seen.length), (null, 1));

      back();
      await pumpEventQueue();
      expect(got, same(_ok));
      expect(events, ['pause', 'resume']);
      expect(inner.seen, hasLength(2));
      expect(identical(inner.seen[1].messages, r.messages), isTrue);
    });

    test('five pauses in one turn send it five more times and it succeeds; '
        'network cuts are still bounded', () async {
      final script = <Object>[
        for (var i = 0; i < 5; i++) _Live(),
        _ok,
      ];
      final inner = _Turns(script);
      final turns = ResumingTurns(inner, p, clock: clock);
      ModelTurn? got;
      unawaited(turns.turn(_turnRequest()).then((t) => got = t));
      for (var i = 0; i < 5; i++) {
        await pumpEventQueue();
        leave();
        p.pause();
        await pumpEventQueue();
        expect((script[i] as _Live).aborted, isTrue);
        back();
      }
      await pumpEventQueue();
      expect(got, same(_ok));
      expect(inner.seen, hasLength(6));

      // After five pauses a turn may still be resent after three network
      // cuts in the background, and the fourth is thrown.
      final cuts = _Turns([
        for (var i = 0; i < 5; i++) _Live(),
        for (var i = 0; i < 4; i++) leave,
      ]);
      Object? error;
      unawaited(
        ResumingTurns(cuts, p, clock: clock)
            .turn(_turnRequest())
            .then<void>((_) {}, onError: (Object e) => error = e),
      );
      for (var i = 0; i < 5; i++) {
        await pumpEventQueue();
        leave();
        p.pause();
        await pumpEventQueue();
        back();
      }
      for (var i = 0; i < 3; i++) {
        await pumpEventQueue();
        expect(error, isNull);
        back();
      }
      await pumpEventQueue();
      expect(cuts.seen, hasLength(9));
      expect(error, same(_dns));
    });

    test('a pause in the foreground does nothing', () async {
      final live = _Live();
      final inner = _Turns([live]);
      unawaited(
        ResumingTurns(
          inner,
          p,
          clock: clock,
        ).turn(_turnRequest()).catchError((Object _) => _ok),
      );
      await pumpEventQueue();
      p.pause();
      await pumpEventQueue();
      expect(live.aborted, isFalse);
      expect(p.paused, isFalse);
    });

    test('a turn that starts while paused is not sent until the app is '
        'back', () async {
      leave();
      p.pause();
      final inner = _Turns([_ok]);
      ModelTurn? got;
      unawaited(
        ResumingTurns(
          inner,
          p,
          clock: clock,
        ).turn(_turnRequest()).then((t) => got = t),
      );
      await pumpEventQueue();
      expect(inner.seen, isEmpty);
      back();
      await pumpEventQueue();
      expect(got, same(_ok));
      expect(inner.seen, hasLength(1));
    });

    test('a cancel during a pause hold throws cancelled and calls no '
        'onResume', () async {
      final cancel = Completer<void>();
      final events = <String>[];
      final inner = _Turns([_Live()]);
      final out = ResumingTurns(
        inner,
        p,
        clock: clock,
        onPause: () => events.add('pause'),
        onResume: () => events.add('resume'),
      ).turn(_turnRequest(cancelled: cancel.future));
      await pumpEventQueue();
      leave();
      p.pause();
      await pumpEventQueue();
      expect(events, ['pause']);
      cancel.complete();
      await expectLater(
        out,
        throwsA(
          isA<AgentError>().having(
            (e) => e.kind,
            'kind',
            AgentErrorKind.cancelled,
          ),
        ),
      );
      expect(events, ['pause']);
      expect(inner.seen, hasLength(1));
    });

    test('in MessagesApiRunner a paused turn is added to the history once '
        'and runs no tool twice, the wall clock stopped while held', () async {
      final render = _Counted('render');
      final finish = _Counted('finish', ends: true);
      final inner = _Turns([
        _turn([_use('t1', 'render')]),
        _Live(() => clock.advance(const Duration(seconds: 5))),
        _turn([_use('t2', 'finish')]),
      ]);
      final tb = toolboxWith(clock: clock, extra: [render, finish]);
      RunOutcome? out;
      unawaited(
        MessagesApiRunner(ResumingTurns(inner, p, clock: clock))
            .run(
              const RunInput(
                system: 's',
                first: [
                  {'type': 'text', 'text': 'u'},
                ],
              ),
              tb,
            )
            .then((o) => out = o),
      );
      await pumpEventQueue();
      expect(inner.seen, hasLength(2));
      leave();
      p.pause();
      await pumpEventQueue();
      clock.advance(const Duration(hours: 5));
      await pumpEventQueue();
      expect(out, isNull);
      expect(tb.state.budget.elapsed, const Duration(seconds: 5));

      back();
      await pumpEventQueue();
      expect(out, isA<RunFinished>());
      expect(inner.seen, hasLength(3));
      expect(
        jsonEncode(inner.seen[2].messages),
        jsonEncode(inner.seen[1].messages),
        reason: 'the paused turn left nothing in the history',
      );
      expect((render.calls, finish.calls), (1, 1));
    });
  });

  group('ResumingBackend', () {
    test('a call in flight is aborted by the pause and run again on return, '
        'not counted against maxResumes', () async {
      final inner = _Call();
      final events = <String>[];
      EditResult? got;
      unawaited(
        ResumingBackend(
              inner,
              p,
              maxResumes: 0,
              onPause: () => events.add('pause'),
              onResume: () => events.add('resume'),
            )
            .run(
              const AgentRequest(
                kind: RequestKind.change,
                model: 'm',
                systemPrompt: 's',
                userText: 'u',
              ),
            )
            .then((r) => got = r),
      );
      await pumpEventQueue();
      leave();
      p.pause();
      await pumpEventQueue();
      expect((inner.runs, inner.aborted), (1, 1));
      expect(events, ['pause']);
      inner.live = false;
      clock.advance(const Duration(hours: 5));
      await pumpEventQueue();
      expect(got, isNull);

      back();
      await pumpEventQueue();
      expect(got, same(_result));
      expect(inner.runs, 2);
      expect(events, ['pause', 'resume']);
    });

    test('a cancel during a pause hold throws cancelled', () async {
      final inner = _Call();
      final cancel = Completer<void>();
      final out = ResumingBackend(inner, p).run(
        AgentRequest(
          kind: RequestKind.change,
          model: 'm',
          systemPrompt: 's',
          userText: 'u',
          cancelled: cancel.future,
        ),
      );
      await pumpEventQueue();
      leave();
      p.pause();
      await pumpEventQueue();
      cancel.complete();
      await expectLater(
        out,
        throwsA(
          isA<AgentError>().having(
            (e) => e.kind,
            'kind',
            AgentErrorKind.cancelled,
          ),
        ),
      );
      expect(inner.runs, 1);
    });
  });
}
