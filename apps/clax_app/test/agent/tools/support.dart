import 'dart:async';

import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/platform/clock.dart';

/// A clock that moves only when a test advances it; a [delay] completes
/// once [advance] reaches its end.
class StepClock implements Clock {
  StepClock([DateTime? start]) : _now = start ?? DateTime.utc(2026, 10, 12);

  DateTime _now;
  final _waiters = <(DateTime, Completer<void>)>[];

  /// Delays not yet completed.
  int get waiting => _waiters.length;

  void advance(Duration d) {
    _now = _now.add(d);
    _waiters.removeWhere((w) {
      if (w.$1.isAfter(_now)) return false;
      w.$2.complete();
      return true;
    });
  }

  @override
  DateTime now() => _now;

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    if (d <= Duration.zero) {
      c.complete();
    } else {
      _waiters.add((_now.add(d), c));
    }
    return c.future;
  }
}

/// A Toolbox over a draft of [files] with the file tools (and [extra]),
/// for a run of [kind] on `claude-haiku-5-5`.
Toolbox toolboxWith({
  Map<String, String> files = const {'index.html': '<p>hi</p>'},
  RequestKind kind = RequestKind.change,
  String? title = 'Runs',
  Map<String, Object?>? capabilities,
  int baseVersion = 3,
  RunCaps? caps,
  StepClock? clock,
  List<Tool> extra = const [],
}) {
  final c = clock ?? StepClock();
  final ctx = RunContext(
    kind: kind,
    artifactId: kind == RequestKind.change ? 'a1' : null,
    baseVersion: baseVersion,
    title: title,
    capabilities: capabilities,
    files: files,
    clock: c,
  );
  final state = RunState(
    ctx: ctx,
    draft: Draft(
      files: files,
      title: title,
      capabilities: capabilities,
      baseVersion: baseVersion,
    ),
    budget: RunBudget(
      caps ?? (kind == RequestKind.newApp ? testNewCaps : testChangeCaps),
      'claude-haiku-5-5',
      c,
    ),
  );
  return Toolbox(state, [...fileTools(), ...extra]);
}

/// Small caps the tool tests count against: 8 turns, 3 renders, 32000
/// output tokens, 90 seconds and \$0.50.
const testNewCaps = RunCaps(
  turns: 8,
  renders: 3,
  outputTokens: 32000,
  wallClock: Duration(seconds: 90),
);

/// Small caps the tool tests count against: 6 turns, 2 renders, 20000
/// output tokens, 45 seconds and \$0.50.
const testChangeCaps = RunCaps(
  turns: 6,
  renders: 2,
  outputTokens: 20000,
  wallClock: Duration(seconds: 45),
);
