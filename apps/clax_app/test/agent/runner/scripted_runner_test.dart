import 'dart:async';

import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:flutter_test/flutter_test.dart';

import '../tools/support.dart';

const _input = RunInput(system: 'You change small apps.', first: []);

Toolbox _toolbox({Future<void>? cancelled}) {
  final clock = StepClock();
  const files = {'index.html': '<p>hi</p>'};
  return Toolbox(
    RunState(
      ctx: RunContext(
        kind: RequestKind.change,
        artifactId: 'a1',
        baseVersion: 3,
        title: 'Runs',
        files: files,
        cancelled: cancelled,
        clock: clock,
      ),
      draft: Draft(files: files, title: 'Runs', baseVersion: 3),
      budget: RunBudget(testChangeCaps, 'claude-haiku-5-5', clock),
    ),
    fileTools(),
  );
}

const _calls = [
  ScriptedCall('write_file', {'path': 'a.txt', 'content': 'one'}),
  ScriptedCall('write_file', {'path': 'b.txt', 'content': 'two'}),
];

Future<void> _settle() async {
  for (var i = 0; i < 20; i++) {
    await Future<void>.value();
  }
}

void main() {
  test('a hold keeps the run before its next call until it completes, and '
      'the run goes on from that call', () async {
    final runner = ScriptedRunner((_) => _calls);
    final tb = _toolbox();
    // Held before the second call.
    final second = Completer<void>();
    final atSecond = Completer<void>();
    var holds = 0;
    runner.hold = () {
      holds++;
      if (holds < 2) return Future.value();
      atSecond.complete();
      return second.future;
    };
    final outcome = runner.run(_input, tb);
    await atSecond.future;
    await _settle();
    expect(tb.state.draft.files['a.txt'], 'one');
    expect(tb.state.draft.files.containsKey('b.txt'), isFalse);
    second.complete();
    final o = await outcome;
    expect(o, isA<RunGaveUp>(), reason: 'the script ends without finish');
    expect(tb.state.draft.files['a.txt'], 'one');
    expect(tb.state.draft.files['b.txt'], 'two');
    expect(holds, 2);
    expect(runner.inputs, hasLength(1));
  });

  test('a cancel during a hold ends the run as cancelled', () async {
    final cancel = Completer<void>();
    final runner = ScriptedRunner((_) => _calls);
    final tb = _toolbox(cancelled: cancel.future);
    final held = Completer<void>();
    final entered = Completer<void>();
    runner.hold = () {
      if (!entered.isCompleted) entered.complete();
      return held.future;
    };
    final outcome = runner.run(_input, tb);
    await entered.future;
    cancel.complete();
    expect(await outcome, isA<RunCancelled>());
    expect(tb.state.draft.files.containsKey('a.txt'), isFalse);
  });

  test('without a hold the run plays every call', () async {
    final runner = ScriptedRunner((_) => _calls);
    final tb = _toolbox();
    await runner.run(_input, tb);
    expect(tb.state.draft.files.keys, containsAll(['a.txt', 'b.txt']));
  });
}
