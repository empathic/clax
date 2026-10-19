/// A runner that plays scripted tool calls against the real [Toolbox], for
/// tests and the debug build's scripted agent.
library;

import '../backend.dart';
import '../progress.dart';
import '../runner.dart';
import '../tools/budget.dart';
import '../tools/toolbox.dart';

/// One step of a scripted run: a call of [tool] with [input], preceded by
/// [narration] when given; or, made with [ScriptedCall.fail] or
/// [ScriptedCall.end], a model failure or an ending the runner itself
/// makes, as a real runner does on a refusal or a broken turn.
class ScriptedCall {
  const ScriptedCall(this.tool, [this.input = const {}, this.narration])
    : error = null,
      ending = null;

  /// The model fails: the run throws [error].
  const ScriptedCall.fail(AgentError this.error)
    : tool = '',
      input = const {},
      narration = null,
      ending = null;

  /// The runner ends the run with [ending] without calling a tool.
  const ScriptedCall.end(RunOutcome this.ending)
    : tool = '',
      input = const {},
      narration = null,
      error = null;

  final String tool;
  final Map<String, Object?> input;
  final String? narration;
  final AgentError? error;
  final RunOutcome? ending;

  @override
  String toString() => switch ((error, ending)) {
    (final e?, _) => 'fail($e)',
    (_, final o?) => 'end($o)',
    _ => '$tool($input)',
  };
}

/// Plays the calls [script] gives for each run, in order, on the run's
/// [Toolbox]: every rule (caps, the publish gate, `finish`) is the
/// Toolbox's, as with a model.
///
/// Each call counts as one model turn with no usage, so the turn cap
/// applies; before each one the run's cancel ends it ([RunCancelled]) and
/// a hard cap of the budget stops it ([Toolbox.stopAtCap], with the latest
/// narration). Progress goes to the run's sink as a model's would: the
/// call's [Narration], a [ToolStarted] with [toolLabel], and for
/// `write_file` a [PartialFile] of its whole content. A script that ends
/// before the run did gives up, as a runner whose model stopped calling
/// tools would.
class ScriptedRunner implements AgentRunner {
  ScriptedRunner(
    this.script, {
    this.name = 'scripted',
    this.sendsImages = true,
  });

  /// Plays [runs] one per run, in order; a run past the last one gives up.
  ScriptedRunner.runs(
    List<List<ScriptedCall>> runs, {
    String name = 'scripted',
    bool sendsImages = true,
  }) : this(_next(List.of(runs)), name: name, sendsImages: sendsImages);

  final List<ScriptedCall> Function(RunInput input) script;

  @override
  final String name;

  @override
  final bool sendsImages;

  /// Every run's input, in order.
  final List<RunInput> inputs = [];

  static List<ScriptedCall> Function(RunInput) _next(
    List<List<ScriptedCall>> runs,
  ) =>
      (_) => runs.isEmpty ? const [] : runs.removeAt(0);

  @override
  Future<RunOutcome> run(RunInput input, Toolbox toolbox) async {
    inputs.add(input);
    final state = toolbox.state;
    final cancel = CancelWatch(state.ctx.cancelled);
    final calls = script(input);
    String? narration;
    try {
      for (final c in calls) {
        // A cancel that has already fired is seen after one turn of the
        // microtask queue.
        await Future<void>.value();
        if (state.outcome case final o?) return o;
        if (cancel.isCancelled) return state.outcome ??= const RunCancelled();
        if (state.budget.hardStop case final cap?) {
          return await toolbox.stopAtCap(cap, narration: narration);
        }
        if (c.error case final e?) throw e;
        if (c.ending case final o?) return state.outcome ??= o;
        state.budget.addTurn(const Usage());
        if (c.narration case final n?) {
          narration = n;
          state.progress?.call(Narration(n));
        }
        state.progress?.call(toolStarted(c.tool, c.input));
        if (c.tool == 'write_file') {
          if (c.input case {
            'path': final String path,
            'content': final String content,
          }) {
            state.progress?.call(PartialFile(path, content));
          }
        }
        await toolbox.call(c.tool, c.input);
      }
      await Future<void>.value();
      if (state.outcome case final o?) return o;
      if (cancel.isCancelled) return state.outcome ??= const RunCancelled();
      return state.outcome ??= const RunGaveUp(
        'the agent stopped without finishing',
        answered: false,
      );
    } finally {
      state.budget.endClock();
    }
  }
}
