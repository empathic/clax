/// The tool loop over Messages API turns.
library;

import 'dart:async';
import 'dart:convert';

import '../backend.dart';
import '../progress.dart';
import '../runner.dart';
import '../tools/toolbox.dart';
import 'turn.dart';

/// The user text after a turn that called no tool.
const finishNudge = 'Call finish or give_up.';

/// Runs a [Toolbox] as a loop of streamed model turns (R§4.2):
///
/// - each turn sends the whole history, which is append-only: an assistant
///   turn goes back exactly as it came (thinking blocks and their
///   signatures included), and the tool results, a [finishNudge] or the
///   budget's soft note are appended after it;
/// - the tool calls of a turn run in order, one at a time; their results
///   answer in one user message, in the same order;
/// - each turn's `max_tokens` is the run's output cap left, at most
///   [maxTokens], so a turn cut off (`max_tokens`) has normally spent the
///   cap and the run stops at it; a cut-off turn, or one with a tool call
///   whose input is not JSON, runs none of its tools; while output cap is
///   left it is sent once more, and a second such turn gives up; a
///   `refusal` gives up naming its category;
/// - before each turn a hard cap of the budget, or the run's cancel,
///   ends the run; while a turn streams, the wall clock running out
///   ([RunBudget.whenTimeUp]) aborts it and the run stops at that cap
///   ([Toolbox.stopAtCap], with the latest [Narration]);
/// - progress goes to `toolbox.state.progress`: each turn's
///   [Narration], [ToolStarted], [PartialFile], [PartialEdit] and the
///   run's [OutputTokens]; an [AttemptStarted] precedes a resent turn and
///   follows a mid-output fallback to another model, after which the
///   partial output starts again from empty;
/// - a turn held back by its backend ([TurnHolds], for example
///   `ResumingTurns` waiting out a background network cut) does not spend
///   the wall clock while held, and is followed by an [AttemptStarted]
///   when it is sent again; a failed turn is never counted and left
///   nothing in the history, so the resent turn is the same request;
/// - a turn is priced per model attempt (a fallback model at its own
///   rates).
class MessagesApiRunner implements AgentRunner {
  MessagesApiRunner(this.backend);

  final TurnBackend backend;

  /// The most `max_tokens` of one turn.
  static const maxTokens = 32000;

  @override
  String get name => backend.name;

  @override
  bool get sendsImages => true;

  @override
  Future<RunOutcome> run(RunInput input, Toolbox toolbox) =>
      _Run(backend, input, toolbox).run();
}

/// Why a turn was stopped from outside.
enum _Stop { cancelled, timeUp }

class _Run {
  _Run(this.backend, this.input, this.toolbox)
    : state = toolbox.state,
      tools = [for (final s in toolbox.specs) s.toMessagesApi()];

  final TurnBackend backend;
  final RunInput input;
  final Toolbox toolbox;
  final RunState state;
  final List<Map<String, Object?>> tools;
  final messages = <Map<String, Object?>>[];

  /// What has stopped the run from outside, first come.
  final _stop = Completer<_Stop>();
  String? _narration;
  var _attempt = 1;
  late final _holds = TurnHolds(
    pauseClock: state.budget.pauseClock,
    resumeClock: state.budget.resumeClock,
    onResend: () => _emit(AttemptStarted(++_attempt)),
  );

  bool get _cancelled => _stopReason == _Stop.cancelled;
  _Stop? _stopReason;

  void _fire(_Stop why) {
    if (_stop.isCompleted) return;
    _stopReason = why;
    _stop.complete(why);
  }

  void _emit(AgentProgress event) {
    if (event case Narration(:final text)) _narration = text;
    if (_cancelled) return;
    // Progress is advisory: a listener that throws never ends the run.
    try {
      state.progress?.call(event);
    } on Object catch (_) {}
  }

  Future<RunOutcome> run() async {
    state.ctx.cancelled?.then(
      (_) => _fire(_Stop.cancelled),
      onError: (Object _) => _fire(_Stop.cancelled),
    );
    final budget = state.budget;
    budget.whenTimeUp.then((_) => _fire(_Stop.timeUp));
    try {
      return await _loop();
    } finally {
      // Whatever ended the run, its deadline no longer matters.
      budget.endClock();
    }
  }

  Future<RunOutcome> _loop() async {
    messages.add({'role': 'user', 'content': input.first});
    var resent = false;
    while (true) {
      if (state.outcome case final o?) return o;
      if (_cancelled) return _end(const RunCancelled());
      if (state.budget.hardStop case final cap?) return _cap(cap);

      final ModelTurn turn;
      try {
        turn = await backend.turn(
          TurnRequest(
            model: state.budget.model,
            system: input.system,
            tools: tools,
            messages: List.unmodifiable(messages),
            maxTokens: _maxTokens(),
            outputCap: state.budget.caps.outputTokens,
            cancelled: _stop.future,
            onProgress: _emit,
            outputBefore: state.budget.usage.output,
            onFallback: () => _emit(AttemptStarted(++_attempt)),
            holds: _holds,
          ),
        );
      } on AgentError catch (e) {
        if (e.kind != AgentErrorKind.cancelled || !_stop.isCompleted) {
          rethrow;
        }
        if (_cancelled) return _end(const RunCancelled());
        return _cap(state.budget.hardStop ?? _wallClockCap());
      }
      state.budget.addTurn(turn.usage, attempts: turn.attempts);

      if (turn.stopReason == 'refusal') {
        final category = switch (turn.stopDetails?['category']) {
          final String c => c,
          _ => 'no category given',
        };
        return _end(
          RunGaveUp('The model declined: $category', answered: false),
        );
      }
      if (turn.broken || turn.stopReason == 'max_tokens') {
        // None of a cut-off or broken turn is used. With the output cap
        // spent the run stops there; else the turn is sent once more.
        if (state.budget.hardStop case final cap?) return _cap(cap);
        if (resent) {
          return _end(
            RunGaveUp(
              turn.stopReason == 'max_tokens'
                  ? 'the answer was cut off twice'
                  : 'the model wrote a tool call that is not valid JSON',
              answered: false,
            ),
          );
        }
        resent = true;
        _emit(AttemptStarted(++_attempt));
        continue;
      }
      resent = false;

      if (turn.content.isNotEmpty) {
        messages.add({'role': 'assistant', 'content': turn.content});
      }
      final uses = [
        for (final b in turn.content)
          if (b['type'] == 'tool_use') b,
      ];
      if (uses.isEmpty) {
        if (turn.stopReason == 'pause_turn') continue;
        messages.add({
          'role': 'user',
          'content': [
            {'type': 'text', 'text': finishNudge},
          ],
        });
        continue;
      }

      final results = <Map<String, Object?>>[];
      for (final use in uses) {
        if (state.outcome case final o?) return o;
        if (_cancelled) return _end(const RunCancelled());
        final out = await toolbox.call('${use['name']}', switch (use['input']) {
          final Map<String, Object?> m => m,
          _ => const {},
        });
        results.add(_result('${use['id']}', out));
      }
      if (state.outcome case final o?) return o;
      if (_cancelled) return _end(const RunCancelled());
      messages.add({
        'role': 'user',
        'content': [
          ...results,
          if (state.budget.softNote case final note?)
            {'type': 'text', 'text': note},
        ],
      });
    }
  }

  /// The output cap left, at most [MessagesApiRunner.maxTokens]; the loop
  /// asks only while some is left.
  int _maxTokens() {
    final left = state.budget.caps.outputTokens - state.budget.usage.output;
    return left.clamp(1, MessagesApiRunner.maxTokens);
  }

  String _wallClockCap() => '${state.budget.caps.wallClock.inSeconds} seconds';

  Future<RunOutcome> _cap(String cap) =>
      toolbox.stopAtCap(cap, narration: _narration);

  RunOutcome _end(RunOutcome o) => state.outcome ??= o;

  static Map<String, Object?> _result(String id, ToolOutput out) =>
      switch (out) {
        ToolOk(:final json, :final png) => {
          'type': 'tool_result',
          'tool_use_id': id,
          'content': [
            {'type': 'text', 'text': jsonEncode(json)},
            if (png != null)
              {
                'type': 'image',
                'source': {
                  'type': 'base64',
                  'media_type': 'image/png',
                  'data': base64Encode(png),
                },
              },
          ],
        },
        ToolRefused(:final message) => {
          'type': 'tool_result',
          'tool_use_id': id,
          'is_error': true,
          'content': [
            {'type': 'text', 'text': message},
          ],
        },
      };
}
