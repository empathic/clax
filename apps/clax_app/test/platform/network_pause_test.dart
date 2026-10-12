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
import 'package:clax_app/platform/network_pause.dart';
import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';

import '../agent/tools/support.dart';

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

/// Answers each turn with the next item: a [ModelTurn] is returned, an
/// [AgentError] thrown, a callback runs first and then [_dns] is thrown.
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
    if (item is void Function()) {
      item();
      throw _dns;
    }
    if (item is AgentError) throw item;
    return item as ModelTurn;
  }
}

/// Counts what a run's clock hooks were asked.
class _Clock {
  var pauses = 0;
  var resumes = 0;
  bool get paused => pauses > resumes;
}

TurnRequest _turnRequest({
  Future<void>? cancelled,
  TurnHolds? holds,
}) => TurnRequest(
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

TurnHolds _holds(_Clock c, {void Function()? onResend}) => TurnHolds(
  pauseClock: () => c.pauses++,
  resumeClock: () => c.resumes++,
  onResend: onResend,
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

  test('a turn cut while backgrounded waits for the foreground with the run '
      'clock stopped, then is sent again unchanged', () async {
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

    clock.advance(const Duration(minutes: 3));
    await pumpEventQueue();
    expect(got, isNull, reason: 'still in the background');

    back();
    await pumpEventQueue();
    expect(events, ['pause', 'resume', 'resend']);
    expect((c.pauses, c.resumes), (1, 1));
    expect(inner.seen, [same(r), same(r)]);
    expect(got, same(_ok));
    expect(r.holds!.held, const Duration(minutes: 3));
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

  test('a turn is resumed at most 3 times', () async {
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

  test('holds of one run share maxHeld; past it the cut is thrown', () async {
    final c = _Clock();
    final holds = _holds(c);
    final turns = ResumingTurns(
      _Turns([leave, _ok, leave]),
      p,
      clock: clock,
      maxHeld: const Duration(minutes: 10),
    );

    final first = turns.turn(_turnRequest(holds: holds));
    await pumpEventQueue();
    clock.advance(const Duration(minutes: 7));
    back();
    expect(await first, same(_ok));
    expect(holds.held, const Duration(minutes: 7));

    // The run's next turn may hold only the 3 minutes left.
    Object? failed;
    unawaited(
      turns.turn(_turnRequest(holds: holds)).catchError((Object e) {
        failed = e;
        return _ok;
      }),
    );
    await pumpEventQueue();
    clock.advance(const Duration(minutes: 2, seconds: 59));
    await pumpEventQueue();
    expect(failed, isNull);
    clock.advance(const Duration(seconds: 1));
    await pumpEventQueue();
    expect(failed, same(_dns));
    expect(holds.held, const Duration(minutes: 10));
    expect((c.pauses, c.resumes), (2, 2));
  });

  test('a run whose holds are spent throws the next cut at once', () async {
    final holds = TurnHolds(pauseClock: () {}, resumeClock: () {})
      ..held = const Duration(minutes: 10);
    final inner = _Turns([leave]);
    await expectLater(
      ResumingTurns(inner, p, clock: clock).turn(_turnRequest(holds: holds)),
      throwsA(same(_dns)),
    );
  });

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
          budget: RunBudget(RunCaps.change, 'claude-haiku-5-5', clock),
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
      clock.advance(const Duration(minutes: 5));
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

    test('a hold past maxHeld fails the run with the network cut', () async {
      final inner = _Turns([leave]);
      final tb = toolbox([_Counted('finish', ends: true)]);
      final out = run(inner, tb);
      await pumpEventQueue();
      clock.advance(ResumingTurns.defaultMaxHeld);
      await expectLater(out, throwsA(same(_dns)));
      expect(tb.state.budget.elapsed, Duration.zero);
    });
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
