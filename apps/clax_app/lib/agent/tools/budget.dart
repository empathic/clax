/// A run's caps and what it has spent against them.
library;

import 'dart:async';

import '../../platform/clock.dart';
import '../pricing.dart';

/// The limits of one run. A run that reaches a hard cap stops calling the
/// model; renders past [renders] are refused one call at a time instead.
class RunCaps {
  const RunCaps({
    required this.turns,
    required this.renders,
    required this.outputTokens,
    this.costUsd = 0.50,
    required this.wallClock,
  });

  /// A new app's first run: plan, then a usable first version.
  static const first = RunCaps(
    turns: 14,
    renders: 4,
    outputTokens: 48000,
    costUsd: 1.50,
    wallClock: Duration(seconds: 150),
  );

  /// A continue run of a build: the next features of the plan.
  static const continueBuild = RunCaps(
    turns: 18,
    renders: 5,
    outputTokens: 64000,
    costUsd: 1.50,
    wallClock: Duration(seconds: 240),
  );

  /// A change in answer to comments.
  static const change = RunCaps(
    turns: 10,
    renders: 3,
    outputTokens: 32000,
    costUsd: 1.50,
    wallClock: Duration(seconds: 120),
  );

  /// Model turns.
  final int turns;

  /// `render` calls.
  final int renders;

  /// Output tokens summed over the run; also the task budget sent with
  /// each Messages API turn.
  final int outputTokens;

  /// The cost safety net, in US dollars.
  final double costUsd;

  /// Wall clock, not counting the time an `ask` waits for the person.
  final Duration wallClock;
}

/// Token counts of one turn or a whole run, as the Messages API's `usage`
/// reports them.
class Usage {
  const Usage({
    this.input = 0,
    this.cacheWrite = 0,
    this.cacheRead = 0,
    this.output = 0,
  });

  /// Reads a Messages API `usage` object; missing or non-integer counts
  /// are 0.
  factory Usage.fromApi(Object? usage) {
    int n(String key) => switch (usage) {
      final Map<Object?, Object?> m when m[key] is int => m[key] as int,
      _ => 0,
    };
    return Usage(
      input: n('input_tokens'),
      cacheWrite: n('cache_creation_input_tokens'),
      cacheRead: n('cache_read_input_tokens'),
      output: n('output_tokens'),
    );
  }

  /// Uncached input tokens.
  final int input;
  final int cacheWrite;
  final int cacheRead;
  final int output;

  Usage operator +(Usage o) => Usage(
    input: input + o.input,
    cacheWrite: cacheWrite + o.cacheWrite,
    cacheRead: cacheRead + o.cacheRead,
    output: output + o.output,
  );

  @override
  bool operator ==(Object other) =>
      other is Usage &&
      other.input == input &&
      other.cacheWrite == cacheWrite &&
      other.cacheRead == cacheRead &&
      other.output == output;

  @override
  int get hashCode => Object.hash(input, cacheWrite, cacheRead, output);

  @override
  String toString() =>
      'Usage(input: $input, cacheWrite: $cacheWrite, '
      'cacheRead: $cacheRead, output: $output)';
}

/// The text appended to a tool-result message once a run nears a cap.
const softNoteText = 'Time is nearly up: call finish now with what works.';

/// What a run of [model] has spent against [caps], timed on [clock] from
/// its construction.
class RunBudget {
  RunBudget(this.caps, this.model, this.clock) : _since = clock.now();

  final RunCaps caps;
  final String model;
  final Clock clock;

  var _turns = 0;
  var _renders = 0;
  var _usage = const Usage();
  var _cost = 0.0;
  var _elapsed = Duration.zero;
  DateTime? _since;
  var _pauses = 0;
  Completer<void>? _resumed;
  Future<void>? _whenTimeUp;
  final _ended = Completer<void>();

  /// Counts one model turn and its usage [u]. The turn is priced as one
  /// request of [model], or, given [attempts] (each model that ran in the
  /// turn, with its own usage, which sum to [u]), as one request per
  /// attempt at that attempt's model.
  void addTurn(Usage u, {List<({String model, Usage usage})>? attempts}) {
    _turns++;
    _usage += u;
    if (attempts == null || attempts.isEmpty) {
      _cost += costOf(model, u);
    } else {
      for (final a in attempts) {
        _cost += costOf(a.model, a.usage);
      }
    }
  }

  /// Counts one render.
  void addRender() => _renders++;

  /// Stops the wall clock, while an `ask` waits for the person or a turn
  /// is held. Pauses nest: the clock runs again at the matching
  /// [resumeClock]. Given [since] (a turn found stalled since then), the
  /// clock is taken to have stopped at [since]: the time from it to now is
  /// not counted, as long as the clock was running then.
  void pauseClock({DateTime? since}) {
    if (_ended.isCompleted) return;
    if (_pauses++ == 0) {
      final now = clock.now();
      final stop = since == null || since.isAfter(now) ? now : since;
      if (_since case final from?) {
        if (stop.isAfter(from)) _elapsed += stop.difference(from);
      }
      _since = null;
      _resumed = Completer<void>();
    }
  }

  /// Ends one [pauseClock].
  void resumeClock() {
    if (_ended.isCompleted || _pauses == 0) return;
    if (--_pauses == 0) {
      _since = clock.now();
      _resumed?.complete();
      _resumed = null;
    }
  }

  /// The wall clock left, paused time excluded; zero once it is spent.
  Duration get remaining {
    final left = caps.wallClock - elapsed;
    return left.isNegative ? Duration.zero : left;
  }

  /// Completes once the wall clock is spent ([timeUp]); it never runs out
  /// while the clock is paused. A runner cancels the turn in flight when it
  /// completes and stops at the cap.
  Future<void> get whenTimeUp => _whenTimeUp ??= _waitTimeUp();

  Future<void> _waitTimeUp() async {
    while (!timeUp) {
      if (_ended.isCompleted) {
        // Ended before the deadline: never completes.
        await Completer<void>().future;
      }
      await Future.any([
        _resumed?.future ?? clock.delay(remaining),
        _ended.future,
      ]);
    }
  }

  /// Ends the run's wall clock for good, when the run is over: [elapsed]
  /// stays where it is, pauses no longer count, and a [whenTimeUp] not
  /// yet complete never completes, so nothing waits on a deadline of a
  /// run that has ended. The clock's own pending delay, if any, then
  /// wakes nothing.
  void endClock() {
    if (_ended.isCompleted) return;
    _elapsed = elapsed;
    _since = null;
    _pauses = 0;
    _resumed?.complete();
    _resumed = null;
    _ended.complete();
  }

  /// The run's wall-clock time so far, paused time left out.
  Duration get elapsed => switch (_since) {
    null => _elapsed,
    final since => _elapsed + clock.now().difference(since),
  };

  int get turns => _turns;
  int get renders => _renders;
  Usage get usage => _usage;

  /// The run's cost so far, each turn priced as its own request.
  double get costUsd => _cost;

  /// Whether another render is within [RunCaps.renders].
  bool get canRender => _renders < caps.renders;

  /// Whether the wall clock is spent.
  bool get timeUp => elapsed >= caps.wallClock;

  /// The first hard cap reached, as the reply names it ("6 turns", "45
  /// seconds", "20000 output tokens", r"$0.50"), or null. The turn cap is
  /// reached once the last allowed turn is counted, so the runner checks
  /// this before each model call.
  String? get hardStop {
    if (_turns >= caps.turns) return '${caps.turns} turns';
    if (timeUp) return '${caps.wallClock.inSeconds} seconds';
    if (_usage.output >= caps.outputTokens) {
      return '${caps.outputTokens} output tokens';
    }
    if (_atLeast(costUsd, caps.costUsd)) {
      return '\$${caps.costUsd.toStringAsFixed(2)}';
    }
    return null;
  }

  /// The cap most nearly spent, named as [hardStop] names it ("10 turns",
  /// "3 renders", …): the hard cap reached, else the one with the largest
  /// share spent.
  String get mostSpent {
    if (hardStop case final cap?) return cap;
    final spent = <String, double>{
      '${caps.turns} turns': _turns / caps.turns,
      '${caps.renders} renders': _renders / caps.renders,
      '${caps.wallClock.inSeconds} seconds':
          elapsed.inMicroseconds / caps.wallClock.inMicroseconds,
      '${caps.outputTokens} output tokens': _usage.output / caps.outputTokens,
      '\$${caps.costUsd.toStringAsFixed(2)}': costUsd / caps.costUsd,
    };
    return spent.entries.reduce((a, b) => b.value > a.value ? b : a).key;
  }

  /// [softNoteText] once two turns or fewer remain, or 60% of the wall
  /// clock, the output tokens or the cost is spent; else null.
  String? get softNote {
    final near =
        caps.turns - _turns <= 2 ||
        elapsed * 10 >= caps.wallClock * 6 ||
        _usage.output * 10 >= caps.outputTokens * 6 ||
        _atLeast(costUsd, caps.costUsd * 0.6);
    return near ? softNoteText : null;
  }

  /// [a] >= [b], forgiving the rounding of the price arithmetic.
  static bool _atLeast(double a, double b) => a >= b - 1e-12;
}
