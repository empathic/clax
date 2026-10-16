import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/messages_api_backend.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/messages_runner.dart';
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/tool_spec.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/platform/connection.dart';
import 'package:clax_app/platform/network_pause.dart';
import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';

import '../agent/tools/support.dart';
import '../support/fake_messages_api.dart';

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

const _dns = AgentError(
  "Couldn't reach the Claude API: SocketException: Failed host lookup: "
  "'api.anthropic.com' (OS Error: No address associated with hostname, "
  'errno = 7)',
  retryable: true,
  kind: AgentErrorKind.network,
);

/// Answers each run with the next item: an [AgentError] is thrown, a
/// callback runs first (to change the app's presence mid-run).
class _Backend implements AgentBackend {
  _Backend(this.script);
  final List<Object> script;
  int runs = 0;

  @override
  String get name => 'fake';

  @override
  Future<EditResult> run(AgentRequest request) async {
    final item = script[runs++];
    if (item is void Function()) {
      item();
      throw _dns;
    }
    if (item is AgentError) throw item;
    return item as EditResult;
  }
}

AgentRequest _request([Future<void>? cancelled]) => AgentRequest(
  kind: RequestKind.newApp,
  model: 'm',
  systemPrompt: 's',
  userText: 'u',
  cancelled: cancelled,
);

void main() {
  test('presence counts leaves and wakes waiters on return', () async {
    final p = AppPresence();
    expect(p.foreground, isTrue);
    p.onLifecycle(AppLifecycleState.inactive);
    expect((p.foreground, p.leaves), (true, 0));
    p.onLifecycle(AppLifecycleState.hidden);
    p.onLifecycle(AppLifecycleState.paused);
    expect((p.foreground, p.leaves), (false, 1));
    var back = false;
    unawaited(p.whenForeground.then((_) => back = true));
    await pumpEventQueue();
    expect(back, isFalse);
    p.onLifecycle(AppLifecycleState.resumed);
    await pumpEventQueue();
    expect(back, isTrue);
  });

  test('network failures are told by their kind, not their text', () {
    expect(isNetworkCut(_dns), isTrue);
    expect(
      isNetworkCut(
        const AgentError(
          "HTTP 500: Couldn't reach the Claude API: SocketException",
        ),
      ),
      isFalse,
    );
    expect(isNetworkCut(const AgentError('The API key was refused')), isFalse);
    expect(isNetworkCut(const AgentError.cancelled()), isFalse);
  });

  test('MessagesApiBackend reports an unreachable API as a network cut, '
      'which ResumingBackend waits out', () async {
    // A port nothing listens on: bind one, note it, close it.
    final s = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = s.port;
    await s.close();
    final p = AppPresence();
    final api = MessagesApiBackend(
      apiKey: () async => 'sk-ant-test',
      baseUrl: Uri.parse('http://127.0.0.1:$port'),
      // The app leaves while the backend waits to retry.
      delay: (_) async => p.onLifecycle(AppLifecycleState.paused),
    );
    final direct = await api
        .run(_request())
        .then<Object?>((_) => null, onError: (Object e) => e);
    expect(direct, isA<AgentError>());
    expect(isNetworkCut(direct! as AgentError), isTrue);

    p.onLifecycle(AppLifecycleState.resumed);
    final cancel = Completer<void>();
    var paused = false;
    final run = ResumingBackend(
      api,
      p,
      onPause: () {
        paused = true;
        cancel.complete();
      },
    ).run(_request(cancel.future));
    await expectLater(
      run,
      throwsA(
        isA<AgentError>().having(
          (e) => e.kind,
          'kind',
          AgentErrorKind.cancelled,
        ),
      ),
    );
    expect(paused, isTrue);
  });

  test('a cut while backgrounded pauses until the app is back, then runs '
      'again', () async {
    final p = AppPresence();
    final events = <String>[];
    final inner = _Backend([
      () => p.onLifecycle(AppLifecycleState.paused),
      _result,
    ]);
    final b = ResumingBackend(
      inner,
      p,
      onPause: () => events.add('pause'),
      onResume: () => events.add('resume'),
    );
    EditResult? got;
    unawaited(b.run(_request()).then((r) => got = r));
    await pumpEventQueue();
    expect(events, ['pause']);
    expect((inner.runs, got), (1, null));
    p.onLifecycle(AppLifecycleState.resumed);
    await pumpEventQueue();
    expect(events, ['pause', 'resume']);
    expect(inner.runs, 2);
    expect(got, same(_result));
  });

  test('a cut seen after the app came back still runs again at once', () async {
    final p = AppPresence();
    final inner = _Backend([
      () {
        p.onLifecycle(AppLifecycleState.paused);
        p.onLifecycle(AppLifecycleState.resumed);
      },
      _result,
    ]);
    expect(await ResumingBackend(inner, p).run(_request()), same(_result));
    expect(inner.runs, 2);
  });

  test('a cut while the app stayed in front fails as before', () async {
    final inner = _Backend([_dns]);
    await expectLater(
      ResumingBackend(inner, AppPresence()).run(_request()),
      throwsA(same(_dns)),
    );
    expect(inner.runs, 1);
  });

  test('other failures are not resumed', () async {
    final p = AppPresence()..onLifecycle(AppLifecycleState.paused);
    const refused = AgentError('The API key was refused');
    await expectLater(
      ResumingBackend(_Backend([refused]), p).run(_request()),
      throwsA(same(refused)),
    );
  });

  test('resumes stop after maxResumes', () async {
    final p = AppPresence();
    void leaveAndReturn() {
      p.onLifecycle(AppLifecycleState.paused);
      p.onLifecycle(AppLifecycleState.resumed);
    }

    final inner = _Backend([leaveAndReturn, leaveAndReturn, leaveAndReturn]);
    await expectLater(
      ResumingBackend(inner, p, maxResumes: 2).run(_request()),
      throwsA(same(_dns)),
    );
    expect(inner.runs, 3);
  });

  test('a cancel while paused ends the run as cancelled', () async {
    final p = AppPresence();
    final cancel = Completer<void>();
    final inner = _Backend([() => p.onLifecycle(AppLifecycleState.paused)]);
    final run = ResumingBackend(inner, p).run(_request(cancel.future));
    await pumpEventQueue();
    cancel.complete();
    await expectLater(
      run,
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

  group('ResumingTurns', turnTests);
  group('offline', offlineTests);
}

// ResumingTurns: the same rule, per turn of a tool loop.

ModelTurn _turn(List<Map<String, Object?>> content, {int output = 10}) =>
    ModelTurn(
      content: content,
      stopReason: content.any((b) => b['type'] == 'tool_use')
          ? 'tool_use'
          : 'end_turn',
      usage: Usage(input: 100, output: output),
    );

final _ok = _turn([
  {'type': 'text', 'text': 'Done.'},
]);

/// A turn whose stream sends one event (after [before] runs), then nothing
/// until it is cancelled; [live] sends another event.
class _Stall {
  _Stall([this.before]);
  final void Function()? before;
  void Function()? live;
}

/// Answers each turn with the next item: a [ModelTurn] is returned, an
/// [AgentError] thrown, a [_Stall] stalls, a callback runs first and then
/// [_dns] is thrown.
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
    if (item is _Stall) {
      r.onActivity?.call();
      item.live = r.onActivity;
      item.before?.call();
      await r.cancelled;
      throw const AgentError.cancelled();
    }
    if (item is void Function()) {
      item();
      throw _dns;
    }
    if (item is AgentError) throw item;
    return item as ModelTurn;
  }
}

/// Records what a run's clock hooks were asked.
class _Clock {
  var pauses = 0;
  var resumes = 0;
  final since = <DateTime?>[];
  bool get paused => pauses > resumes;
}

TurnRequest _turnRequest({Future<void>? cancelled, TurnHolds? holds}) =>
    TurnRequest(
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
      holds: holds,
    );

TurnHolds _holds(
  _Clock c, {
  void Function()? onResend,
  void Function(bool waiting)? onConnection,
}) => TurnHolds(
  pauseClock: ({since}) {
    c.pauses++;
    c.since.add(since);
  },
  resumeClock: () => c.resumes++,
  onResend: onResend,
  onConnection: onConnection,
);

void turnTests() {
  late AppPresence p;
  late StepClock clock;

  setUp(() {
    p = AppPresence();
    clock = StepClock();
  });

  void leave() => p.onLifecycle(AppLifecycleState.paused);
  void back() => p.onLifecycle(AppLifecycleState.resumed);

  void sameRequest(TurnRequest a, TurnRequest b) {
    expect(identical(a.messages, b.messages), isTrue);
    expect((a.model, a.system, a.maxTokens), (b.model, b.system, b.maxTokens));
    expect(identical(a.holds, b.holds), isTrue);
  }

  test('a turn cut while backgrounded waits for the foreground with the run '
      'clock stopped, however long, then is sent again unchanged', () async {
    final c = _Clock();
    final events = <String>[];
    final inner = _Turns([leave, _ok]);
    final turns = ResumingTurns(
      inner,
      p,
      clock: clock,
      onPause: () => events.add('pause'),
      onResume: () => events.add('resume'),
    );
    final r = _turnRequest(
      holds: _holds(c, onResend: () => events.add('resend')),
    );
    ModelTurn? got;
    unawaited(turns.turn(r).then((t) => got = t));
    await pumpEventQueue();
    expect(events, ['pause']);
    expect(c.paused, isTrue);
    expect((inner.seen.length, got), (1, null));

    clock.advance(const Duration(hours: 5));
    await pumpEventQueue();
    expect(got, isNull, reason: 'a hold has no time limit');

    back();
    await pumpEventQueue();
    expect(events, ['pause', 'resume', 'resend']);
    expect((c.pauses, c.resumes), (1, 1));
    expect(inner.seen, hasLength(2));
    sameRequest(inner.seen[0], r);
    sameRequest(inner.seen[1], r);
    expect(got, same(_ok));
  });

  test('a cut seen after the app came back is sent again at once', () async {
    final c = _Clock();
    final inner = _Turns([
      () {
        leave();
        back();
      },
      _ok,
    ]);
    final r = _turnRequest(holds: _holds(c));
    expect(await ResumingTurns(inner, p, clock: clock).turn(r), same(_ok));
    expect(inner.seen, hasLength(2));
    expect((c.pauses, c.resumes), (1, 1));
  });

  test('a cut while the app stayed in front is thrown', () async {
    final c = _Clock();
    final inner = _Turns([_dns]);
    await expectLater(
      ResumingTurns(
        inner,
        p,
        clock: clock,
      ).turn(_turnRequest(holds: _holds(c))),
      throwsA(same(_dns)),
    );
    expect(inner.seen, hasLength(1));
    expect(c.pauses, 0);
  });

  test('other failures are not resumed', () async {
    leave();
    const refused = AgentError('The API key was refused');
    final inner = _Turns([refused]);
    await expectLater(
      ResumingTurns(inner, p, clock: clock).turn(_turnRequest()),
      throwsA(same(refused)),
    );
    expect(inner.seen, hasLength(1));
  });

  test('a turn is sent again at most 3 times', () async {
    void leaveAndReturn() {
      leave();
      back();
    }

    final inner = _Turns([
      leaveAndReturn,
      leaveAndReturn,
      leaveAndReturn,
      leaveAndReturn,
    ]);
    await expectLater(
      ResumingTurns(inner, p, clock: clock).turn(_turnRequest()),
      throwsA(same(_dns)),
    );
    expect(inner.seen, hasLength(4));
  });

  test('the limit is per turn: the next turn of a run may be resent '
      'again', () async {
    void leaveAndReturn() {
      leave();
      back();
    }

    final c = _Clock();
    final holds = _holds(c);
    final inner = _Turns([
      leaveAndReturn,
      leaveAndReturn,
      leaveAndReturn,
      _ok,
      leaveAndReturn,
      _ok,
    ]);
    final turns = ResumingTurns(inner, p, clock: clock);
    expect(await turns.turn(_turnRequest(holds: holds)), same(_ok));
    expect(await turns.turn(_turnRequest(holds: holds)), same(_ok));
    expect(inner.seen, hasLength(6));
    expect((c.pauses, c.resumes), (4, 4));
  });

  test('a cancel while held ends the turn as cancelled, the clock running '
      'again', () async {
    final c = _Clock();
    final cancel = Completer<void>();
    final inner = _Turns([leave]);
    final turn = ResumingTurns(
      inner,
      p,
      clock: clock,
    ).turn(_turnRequest(cancelled: cancel.future, holds: _holds(c)));
    await pumpEventQueue();
    expect(c.paused, isTrue);
    cancel.complete();
    await expectLater(
      turn,
      throwsA(
        isA<AgentError>().having(
          (e) => e.kind,
          'kind',
          AgentErrorKind.cancelled,
        ),
      ),
    );
    expect(inner.seen, hasLength(1));
    expect((c.pauses, c.resumes), (1, 1));
  });

  test('a throwing onPause still leaves the run clock running', () async {
    final c = _Clock();
    final inner = _Turns([leave]);
    final turn = ResumingTurns(
      inner,
      p,
      clock: clock,
      onPause: () => throw StateError('notification failed'),
    ).turn(_turnRequest(holds: _holds(c)));
    await expectLater(turn, throwsStateError);
    expect((c.pauses, c.resumes), (1, 1));
  });

  group('stall', () {
    test('a stream quiet for stallAfter in the background is aborted and '
        'held with the clock stopped from its last event', () async {
      final c = _Clock();
      final start = clock.now();
      final stall = _Stall();
      final inner = _Turns([stall, _ok]);
      ModelTurn? got;
      unawaited(
        ResumingTurns(
          inner,
          p,
          clock: clock,
        ).turn(_turnRequest(holds: _holds(c))).then((t) => got = t),
      );
      await pumpEventQueue();
      clock.advance(const Duration(seconds: 5));
      stall.live!();
      leave();
      clock.advance(const Duration(seconds: 29));
      await pumpEventQueue();
      expect(c.pauses, 0, reason: '29 s quiet is not a stall');

      clock.advance(const Duration(seconds: 1));
      await pumpEventQueue();
      expect(c.pauses, 1);
      expect(c.since, [start.add(const Duration(seconds: 5))]);
      expect(inner.seen, hasLength(1));
      expect(got, isNull);

      back();
      await pumpEventQueue();
      expect(inner.seen, hasLength(2));
      expect(got, same(_ok));
      expect((c.pauses, c.resumes), (1, 1));
    });

    test('a quiet stream in front is left alone', () async {
      final c = _Clock();
      final cancel = Completer<void>();
      final inner = _Turns([_Stall()]);
      final turn = ResumingTurns(
        inner,
        p,
        clock: clock,
      ).turn(_turnRequest(cancelled: cancel.future, holds: _holds(c)));
      await pumpEventQueue();
      for (var i = 0; i < 10; i++) {
        clock.advance(const Duration(minutes: 1));
        await pumpEventQueue();
      }
      expect(c.pauses, 0);
      cancel.complete();
      await expectLater(turn, throwsA(isA<AgentError>()));
      expect(inner.seen, hasLength(1));
    });

    test('a stream that keeps sending events in the background is left '
        'alone', () async {
      final c = _Clock();
      final cancel = Completer<void>();
      final stall = _Stall(leave);
      final turn = ResumingTurns(
        _Turns([stall]),
        p,
        clock: clock,
      ).turn(_turnRequest(cancelled: cancel.future, holds: _holds(c)));
      await pumpEventQueue();
      for (var i = 0; i < 10; i++) {
        clock.advance(const Duration(seconds: 20));
        stall.live!();
        await pumpEventQueue();
      }
      expect(c.pauses, 0);
      cancel.complete();
      await expectLater(turn, throwsA(isA<AgentError>()));
    });

    test('a stall seen only once the app is back (the process was frozen) '
        'still counts: the app left since the last event', () async {
      final c = _Clock();
      final stall = _Stall(leave);
      final inner = _Turns([stall, _ok]);
      final turn = ResumingTurns(
        inner,
        p,
        clock: clock,
      ).turn(_turnRequest(holds: _holds(c)));
      await pumpEventQueue();
      back();
      clock.advance(const Duration(minutes: 4));
      expect(await turn, same(_ok));
      expect(c.since, [isNotNull]);
      expect(inner.seen, hasLength(2));
    });

    test('a cancel while a stall is held throws cancelled', () async {
      final c = _Clock();
      final cancel = Completer<void>();
      final turn = ResumingTurns(
        _Turns([_Stall(leave)]),
        p,
        clock: clock,
      ).turn(_turnRequest(cancelled: cancel.future, holds: _holds(c)));
      await pumpEventQueue();
      clock.advance(const Duration(seconds: 30));
      await pumpEventQueue();
      expect(c.paused, isTrue);
      cancel.complete();
      await expectLater(
        turn,
        throwsA(
          isA<AgentError>().having(
            (e) => e.kind,
            'kind',
            AgentErrorKind.cancelled,
          ),
        ),
      );
      expect((c.pauses, c.resumes), (1, 1));
    });
  });

  test(
    'MessagesApiBackend reports each server-sent event as activity',
    () async {
      final api = await FakeMessagesApi.start([
        sseReply(
          streamedTurn([
            {'type': 'text', 'text': 'Done.'},
          ], stopReason: 'end_turn'),
        ),
      ]);
      addTearDown(api.close);
      var events = 0;
      final backend = MessagesApiBackend(
        apiKey: () async => 'sk-ant-test',
        baseUrl: api.baseUrl,
        delay: (_) async {},
      );
      final r = _turnRequest();
      await backend.turn(
        r.watched(
          cancelled: Completer<void>().future,
          onActivity: () => events++,
        ),
      );
      expect(events, greaterThan(3));
    },
  );

  group('in MessagesApiRunner', () {
    late Completer<void> cancel;
    late List<AgentProgress> events;

    setUp(() {
      cancel = Completer<void>();
      events = [];
    });

    Toolbox toolbox(List<Tool> tools) {
      const files = {'index.html': '<p>runs</p>'};
      return Toolbox(
        RunState(
          ctx: RunContext(
            kind: RequestKind.change,
            artifactId: 'a1',
            baseVersion: 3,
            files: files,
            cancelled: cancel.future,
            clock: clock,
          ),
          draft: Draft(files: files, title: 'Runs', baseVersion: 3),
          budget: RunBudget(testChangeCaps, 'claude-haiku-5-5', clock),
          progress: events.add,
        ),
        tools,
      );
    }

    Future<RunOutcome> run(TurnBackend b, Toolbox tb) =>
        MessagesApiRunner(ResumingTurns(b, p, clock: clock)).run(
          const RunInput(
            system: 's',
            first: [
              {'type': 'text', 'text': 'u'},
            ],
          ),
          tb,
        );

    Map<String, Object?> use(String id, String name) => {
      'type': 'tool_use',
      'id': id,
      'name': name,
      'input': <String, Object?>{},
    };

    test('a resumed turn runs no tool twice, is counted once and pauses the '
        'wall clock while held', () async {
      final render = _Counted('render');
      final finish = _Counted('finish', ends: true);
      final inner = _Turns([
        _turn([use('t1', 'render')]),
        () {
          clock.advance(const Duration(seconds: 5));
          leave();
        },
        _turn([use('t2', 'finish')]),
      ]);
      final tb = toolbox([render, finish]);
      RunOutcome? out;
      unawaited(run(inner, tb).then((o) => out = o));
      await pumpEventQueue();
      expect(inner.seen, hasLength(2));
      expect(render.calls, 1);

      // Far past the 45 s wall clock while held: the run is not stopped.
      clock.advance(const Duration(hours: 1));
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
        reason: 'the cut turn left nothing in the history',
      );
      expect((render.calls, finish.calls), (1, 1));
      expect(tb.state.budget.turns, 2);
      expect(tb.state.budget.usage, const Usage(input: 200, output: 20));
      expect(events.whereType<AttemptStarted>(), hasLength(1));
    });

    test('a stalled background turn is held with the wall clock taken back '
        'to its last event, then sent again', () async {
      final stall = _Stall();
      final inner = _Turns([
        stall,
        _turn([use('t1', 'finish')]),
      ]);
      final tb = toolbox([_Counted('finish', ends: true)]);
      RunOutcome? out;
      unawaited(run(inner, tb).then((o) => out = o));
      await pumpEventQueue();
      clock.advance(const Duration(seconds: 5));
      stall.live!();
      leave();
      // Past the stall (30 s after the last event), short of the 45 s cap.
      clock.advance(const Duration(seconds: 31));
      await pumpEventQueue();
      clock.advance(const Duration(hours: 1));
      await pumpEventQueue();
      expect(out, isNull);
      expect(tb.state.budget.elapsed, const Duration(seconds: 5));

      back();
      await pumpEventQueue();
      expect(out, isA<RunFinished>());
      expect(inner.seen, hasLength(2));
      expect(tb.state.budget.turns, 1);
    });

    test('a cancel while held ends the run cancelled', () async {
      final inner = _Turns([leave]);
      final tb = toolbox([_Counted('finish', ends: true)]);
      final out = run(inner, tb);
      await pumpEventQueue();
      cancel.complete();
      expect(await out, isA<RunCancelled>());
      expect(inner.seen, hasLength(1));
    });

    test(
      'a wall clock spent as the cut arrives stops the run at its cap',
      () async {
        final inner = _Turns([
          () {
            clock.advance(const Duration(seconds: 46));
            leave();
          },
        ]);
        final tb = toolbox([_Counted('finish', ends: true)]);
        // With no cap handler, stopAtCap gives up naming the cap.
        expect(
          await run(inner, tb),
          isA<RunGaveUp>().having((o) => o.reason, 'reason', '45 seconds'),
        );
        expect(inner.seen, hasLength(1));
      },
    );
  });
}

/// A tool that counts its calls; [ends] sets the run's outcome.
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

// Offline: a cut while the app is in front waits for the connection.

void offlineTests() {
  late AppPresence p;
  late StepClock clock;
  late bool reachable;
  late ApiConnection conn;

  setUp(() {
    p = AppPresence();
    clock = StepClock();
    reachable = false;
    conn = ApiConnection(probe: () async => reachable, clock: clock);
  });

  /// The API answers again: the next probe finds it.
  Future<void> reconnect() async {
    reachable = true;
    clock.advance(ApiConnection.maxProbe);
    await pumpEventQueue();
  }

  test('a turn cut in front waits for the connection with the run clock '
      'stopped, however long, then is sent again unchanged', () async {
    final c = _Clock();
    final events = <String>[];
    final inner = _Turns([_dns, _ok]);
    final turns = ResumingTurns(
      inner,
      p,
      clock: clock,
      connection: conn,
      onPause: () => events.add('pause'),
      onOffline: () => events.add('offline'),
      onResume: () => events.add('resume'),
    );
    final r = _turnRequest(
      holds: _holds(
        c,
        onResend: () => events.add('resend'),
        onConnection: (w) => events.add(w ? 'waiting' : 'back'),
      ),
    );
    ModelTurn? got;
    unawaited(turns.turn(r).then((t) => got = t));
    await pumpEventQueue();
    expect(conn.online, isFalse);
    expect(events, ['offline', 'waiting']);
    expect(c.paused, isTrue);

    clock.advance(const Duration(hours: 5));
    await pumpEventQueue();
    expect(got, isNull, reason: 'waiting has no time limit');

    await reconnect();
    expect(events, ['offline', 'waiting', 'back', 'resume', 'resend']);
    expect((c.pauses, c.resumes), (1, 1));
    expect(inner.seen, hasLength(2));
    expect(identical(inner.seen[1].messages, r.messages), isTrue);
    expect(got, same(_ok));
  });

  test('a turn that starts while the API is known unreachable is not sent '
      'until it is back', () async {
    final c = _Clock();
    final events = <String>[];
    conn.lost();
    final inner = _Turns([_ok]);
    ModelTurn? got;
    unawaited(
      ResumingTurns(inner, p, clock: clock, connection: conn)
          .turn(
            _turnRequest(
              holds: _holds(
                c,
                onConnection: (w) => events.add(w ? 'waiting' : 'back'),
              ),
            ),
          )
          .then((t) => got = t),
    );
    await pumpEventQueue();
    expect(inner.seen, isEmpty);
    expect(events, ['waiting']);
    expect(c.paused, isTrue);

    await reconnect();
    expect(inner.seen, hasLength(1));
    expect(got, same(_ok));
    expect(events, ['waiting', 'back']);
    expect((c.pauses, c.resumes), (1, 1));
  });

  test('waits for the connection are not counted against maxResumes', () async {
    final c = _Clock();
    final inner = _Turns([_dns, _dns, _dns, _dns, _dns, _ok]);
    ModelTurn? got;
    unawaited(
      ResumingTurns(
        inner,
        p,
        clock: clock,
        connection: conn,
      ).turn(_turnRequest(holds: _holds(c))).then((t) => got = t),
    );
    for (var i = 0; i < 5; i++) {
      await pumpEventQueue();
      expect(conn.online, isFalse);
      await reconnect();
      reachable = false;
    }
    await pumpEventQueue();
    expect(inner.seen, hasLength(6));
    expect(got, same(_ok));
  });

  test(
    'a cancel while waiting for the connection ends the turn cancelled',
    () async {
      final c = _Clock();
      final cancel = Completer<void>();
      final inner = _Turns([_dns]);
      final turn = ResumingTurns(
        inner,
        p,
        clock: clock,
        connection: conn,
      ).turn(_turnRequest(cancelled: cancel.future, holds: _holds(c)));
      await pumpEventQueue();
      cancel.complete();
      await expectLater(
        turn,
        throwsA(
          isA<AgentError>().having(
            (e) => e.kind,
            'kind',
            AgentErrorKind.cancelled,
          ),
        ),
      );
      expect(inner.seen, hasLength(1));
      expect((c.pauses, c.resumes), (1, 1));
    },
  );

  test('a stream quiet in front for stallAfter is probed: unreachable, it '
      'is aborted and waits for the connection', () async {
    final c = _Clock();
    final stall = _Stall();
    final inner = _Turns([stall, _ok]);
    ModelTurn? got;
    unawaited(
      ResumingTurns(
        inner,
        p,
        clock: clock,
        connection: conn,
      ).turn(_turnRequest(holds: _holds(c))).then((t) => got = t),
    );
    await pumpEventQueue();
    clock.advance(ResumingTurns.defaultStallAfter);
    await pumpEventQueue();
    expect(conn.online, isFalse);
    expect(c.pauses, 1);
    expect(inner.seen, hasLength(1));

    await reconnect();
    expect(inner.seen, hasLength(2));
    expect(got, same(_ok));
  });

  test(
    'a stream quiet in front with the API reachable is left alone',
    () async {
      reachable = true;
      final c = _Clock();
      final cancel = Completer<void>();
      final inner = _Turns([_Stall()]);
      final turn = ResumingTurns(
        inner,
        p,
        clock: clock,
        connection: conn,
      ).turn(_turnRequest(cancelled: cancel.future, holds: _holds(c)));
      for (var i = 0; i < 10; i++) {
        clock.advance(const Duration(minutes: 1));
        await pumpEventQueue();
      }
      expect(c.pauses, 0);
      expect(conn.online, isTrue);
      cancel.complete();
      await expectLater(turn, throwsA(isA<AgentError>()));
      expect(inner.seen, hasLength(1));
    },
  );

  test('ResumingBackend: a run cut in front waits for the connection, then '
      'runs again', () async {
    final events = <String>[];
    final inner = _Backend([_dns, _result]);
    EditResult? got;
    unawaited(
      ResumingBackend(
        inner,
        p,
        connection: conn,
        onOffline: () => events.add('offline'),
        onResume: () => events.add('resume'),
      ).run(_request()).then((r) => got = r),
    );
    await pumpEventQueue();
    expect(events, ['offline']);
    expect(inner.runs, 1);
    await reconnect();
    expect(events, ['offline', 'resume']);
    expect(inner.runs, 2);
    expect(got, same(_result));
  });

  group('in MessagesApiRunner', () {
    late Completer<void> cancel;
    late List<AgentProgress> events;

    setUp(() {
      cancel = Completer<void>();
      events = [];
    });

    Map<String, Object?> use(String id, String name) => {
      'type': 'tool_use',
      'id': id,
      'name': name,
      'input': <String, Object?>{},
    };

    test('a run that loses the connection mid-way waits and resumes: no '
        'tool runs twice, it publishes once, and says it is waiting', () async {
      const files = {'index.html': '<p>runs</p>'};
      final render = _Counted('render');
      final finish = _Counted('finish', ends: true);
      final tb = Toolbox(
        RunState(
          ctx: RunContext(
            kind: RequestKind.change,
            artifactId: 'a1',
            baseVersion: 3,
            files: files,
            cancelled: cancel.future,
            clock: clock,
          ),
          draft: Draft(files: files, title: 'Runs', baseVersion: 3),
          budget: RunBudget(testChangeCaps, 'claude-haiku-5-5', clock),
          progress: events.add,
        ),
        [render, finish],
      );
      final inner = _Turns([
        _turn([use('t1', 'render')]),
        _dns,
        _turn([use('t2', 'finish')]),
      ]);
      RunOutcome? out;
      unawaited(
        MessagesApiRunner(
              ResumingTurns(inner, p, clock: clock, connection: conn),
            )
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
      expect(out, isNull);
      expect(events.whereType<WaitingForConnection>(), hasLength(1));
      clock.advance(const Duration(hours: 1));
      await pumpEventQueue();
      expect(out, isNull, reason: 'the wall clock is stopped while waiting');

      await reconnect();
      expect(out, isA<RunFinished>());
      expect((render.calls, finish.calls), (1, 1));
      expect(inner.seen, hasLength(3));
      expect(
        jsonEncode(inner.seen[2].messages),
        jsonEncode(inner.seen[1].messages),
        reason: 'the cut turn left nothing in the history',
      );
      expect(events.whereType<ConnectionBack>(), hasLength(1));
    });
  });
}
