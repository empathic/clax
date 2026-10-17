/// One run's tools and the rules they share: the draft, the caps and how
/// a run ends.
library;

import 'dart:typed_data';

import '../../clax/models.dart' show FeedbackItem;
import '../../daemon/clock.dart';
import '../failure.dart' show changeCapUsd, limitFailureReply, limitLandedReply;
import '../progress.dart';
import '../read_consent.dart';
import '../request.dart';
import 'budget.dart';
import 'draft.dart';
import 'tool_spec.dart';

/// What a tool call answers.
sealed class ToolOutput {
  const ToolOutput();
}

/// A tool call that did its work: [json] is the result, [png] an image to
/// send with it.
final class ToolOk extends ToolOutput {
  const ToolOk(this.json, {this.png});

  final Map<String, Object?> json;
  final Uint8List? png;

  @override
  String toString() => 'ToolOk($json${png == null ? '' : ', png'})';
}

/// A tool call that was refused, with one sentence the model can act on.
/// It answers as an error `tool_result`; the run goes on.
final class ToolRefused extends ToolOutput {
  const ToolRefused(this.message);

  final String message;

  @override
  String toString() => 'ToolRefused($message)';
}

/// Thrown by a tool on input it cannot use; [Toolbox.call] answers it as a
/// [ToolRefused] with [message].
class ToolInputError implements Exception {
  const ToolInputError(this.message);

  final String message;

  @override
  String toString() => 'ToolInputError: $message';
}

/// One tool of a run.
abstract interface class Tool {
  ToolSpec get spec;

  /// Runs the tool on [input] against the run [s]. Throws
  /// [ToolInputError] for input it refuses; any other [Exception] is also
  /// answered as a refusal.
  Future<ToolOutput> call(Map<String, Object?> input, RunState s);
}

/// What a run starts from: the request and the app as it was.
class RunContext {
  RunContext({
    required this.kind,
    this.artifactId,
    this.baseVersion = 0,
    this.title,
    this.capabilities,
    required this.files,
    this.items = const [],
    this.threadIds = const [],
    this.shownDocs = const {},
    this.sessionId,
    this.cancelled,
    required this.clock,
    this.changeSpentUsd,
    this.changeStops = 0,
    this.changeNotes = const [],
    this.startedOver = false,
  });

  final RequestKind kind;

  /// The app a change is on; null for a new app.
  final String? artifactId;

  /// The version a change started from; 0 for a new app.
  final int baseVersion;
  final String? title;
  final Map<String, Object?>? capabilities;

  /// The base version's text files by path.
  final Map<String, String> files;

  /// The comments a change answers.
  final List<FeedbackItem> items;
  final List<String> threadIds;

  /// Document path → the version the first message showed it at, so a
  /// write may be pinned to it without a read. It holds this app's own
  /// documents only, never another app's samples: `db_write` pins this
  /// app's writes to these versions by path alone.
  final Map<String, int?> shownDocs;

  /// The app session the run replies under.
  final String? sessionId;

  /// Completes when the run is cancelled.
  final Future<void>? cancelled;
  final Clock clock;

  /// For a change in answer to comments: what its earlier runs (the run
  /// Keep going goes on from) cost, against [changeCapUsd]. A run that
  /// ends at or near a limit then replies in Clax's words naming it
  /// ([limitFailureReply], [limitLandedReply]). Null for a new app or a
  /// build's continue run, which reply as before.
  final double? changeSpentUsd;

  /// For a change: how many of its earlier runs stopped at a limit.
  final int changeStops;

  /// For a change: the agent's notes over its earlier runs, oldest first.
  final List<String> changeNotes;

  /// A Keep going that could not go on from the stopped run's draft and
  /// started again from the app: its replies open with [startedOverLine].
  final bool startedOver;
}

/// How a run ended.
sealed class RunOutcome {
  const RunOutcome();
}

/// The run published [version] of [artifactId] (null when nothing
/// changed); [answered] says the threads got their reply.
final class RunFinished extends RunOutcome {
  const RunFinished({
    this.artifactId,
    this.version,
    required this.answered,
    this.docsRefused,
    this.limit,
  });

  final String? artifactId;
  final int? version;
  final bool answered;

  /// The hard cap a cap stop published the last clean draft at ("10
  /// turns"); null for a `finish`.
  final String? limit;

  /// For a new app: why its starting documents were not written (the app
  /// was made without them), or null when they were, or there were none.
  final String? docsRefused;
}

/// The run stopped without publishing, for [reason].
final class RunGaveUp extends RunOutcome {
  const RunGaveUp(
    this.reason, {
    required this.answered,
    this.limit,
    this.planned = false,
  });

  final String reason;
  final bool answered;

  /// The change stopped at a limit after planning what is left
  /// ([plannedStopReply]): the build goes on with it, so it is no failure.
  final bool planned;

  /// The cap a change stopped at, or gave up out of room near
  /// ([RunBudget.mostSpent]), which the person can Keep going from; else
  /// null.
  final String? limit;
}

/// The run was cancelled; nothing was published or replied.
final class RunCancelled extends RunOutcome {
  const RunCancelled();
}

/// The daemon could not be reached mid-run; the change ledger keeps what
/// recovery needs.
final class RunInterrupted extends RunOutcome {
  const RunInterrupted();
}

/// The app or its threads are gone.
final class RunDropped extends RunOutcome {
  const RunDropped();
}

/// What a run's tools share.
class RunState {
  RunState({
    required this.ctx,
    required this.draft,
    required this.budget,
    this.progress,
    this.readConsent,
    List<String>? narrations,
  }) : narrations = narrations ?? [];

  final RunContext ctx;

  /// What the agent narrated this run, in order (each [Narration] the
  /// runner sent to [progress]).
  final List<String> narrations;
  final Draft draft;
  final RunBudget budget;
  final ProgressSink? progress;

  /// Asks the owner before the agent reads another app's documents; null:
  /// such reads are refused.
  final AgentReadConsent? readConsent;

  /// Source app ID → the owner's answer this run, so each app is asked
  /// about at most once a run.
  final Map<String, ConsentAnswer> readConsents = {};

  /// Whether the run's latest `plan_set` left features to build: a stop
  /// at a limit then leaves the rest to the build ([plannedStopReply])
  /// rather than offering Keep going.
  bool planned = false;

  /// Set once by whatever ends the run: `finish`, `give_up`, a cap or a
  /// cancel.
  RunOutcome? outcome;

  bool get ended => outcome != null;
}

/// One run's tools over one [RunState]. Every rule lives here or in the
/// tools, so each runner behaves the same.
class Toolbox {
  /// Orders [tools] by [toolOrder]; a name in [toolOrder] without a tool is
  /// left out. A tool whose name is not in [toolOrder], or two tools with
  /// one name, is an [ArgumentError].
  Toolbox(this.state, List<Tool> tools) : _tools = _byOrder(tools);

  final RunState state;
  final Map<String, Tool> _tools;
  Future<RunOutcome> Function(String cap, String? narration)? _capHandler;

  /// Completes when the latest queued call or cap stop has ended.
  Future<void> _tail = Future.value();

  static Map<String, Tool> _byOrder(List<Tool> tools) {
    final byName = <String, Tool>{};
    for (final t in tools) {
      final name = t.spec.name;
      if (!toolOrder.contains(name)) {
        throw ArgumentError.value(name, 'tools', 'not a tool of toolOrder');
      }
      if (byName.containsKey(name)) {
        throw ArgumentError.value(name, 'tools', 'given twice');
      }
      byName[name] = t;
    }
    return {
      for (final name in toolOrder)
        if (byName.containsKey(name)) name: byName[name]!,
    };
  }

  /// The tools' specs, in [toolOrder].
  List<ToolSpec> get specs => [for (final t in _tools.values) t.spec];

  /// Ends the run at a hard cap: publishes the last clean draft or gives up
  /// (`finish`'s `capStop`).
  set capHandler(
    Future<RunOutcome> Function(String cap, String? narration) h,
  ) => _capHandler = h;

  /// Runs the tool [name] on [input], after every call made before it has
  /// ended: calls run one at a time, in the order they are made, so a
  /// write never lands while a render is inspecting the draft.
  ///
  /// Never throws an [Exception]: an unknown tool, a call made once the
  /// wall clock is spent ([RunBudget.timeUp], `finish` and `give_up`
  /// included), a call that starts after the run ended, refused input and
  /// a tool's exception all answer as [ToolRefused]. A call made before the
  /// deadline runs even when it starts after it.
  ///
  /// Turn, token and cost caps do not refuse calls: the turn that reached
  /// them is paid for, so its tools run, and the runner stops before the
  /// next turn.
  Future<ToolOutput> call(String name, Map<String, Object?> input) {
    final tool = _tools[name];
    if (tool == null) {
      return Future.value(
        ToolRefused(
          'There is no tool "$name"; use one of ${_tools.keys.join(', ')}.',
        ),
      );
    }
    if (state.budget.timeUp) {
      return Future.value(
        ToolRefused(
          'The run is out of time '
          '(${state.budget.caps.wallClock.inSeconds} seconds).',
        ),
      );
    }
    return _queued(() async {
      if (state.ended) return const ToolRefused('The run has ended.');
      try {
        return await tool.call(input, state);
      } on ToolInputError catch (e) {
        return ToolRefused(e.message);
      } on Exception catch (e) {
        return ToolRefused('$name failed: ${_reason(e)}');
      }
    });
  }

  /// Ends the run at the hard cap [cap] ("6 turns", "45 seconds", …), with
  /// the agent's latest [narration], once the calls already made have
  /// ended. The [capHandler] publishes [Draft.cleanOnBase] or gives up;
  /// without one the run gives up naming [cap]. A run that has already
  /// ended keeps its outcome.
  Future<RunOutcome> stopAtCap(String cap, {String? narration}) =>
      _queued(() async {
        if (state.outcome case final o?) return o;
        final out = switch (_capHandler) {
          final h? => await h(cap, narration),
          null => RunGaveUp(cap, answered: false),
        };
        return state.outcome ??= out;
      });

  Future<T> _queued<T>(Future<T> Function() work) {
    final result = _tail.then((_) => work());
    _tail = result.then<void>((_) {}, onError: (Object _) {});
    return result;
  }

  static String _reason(Exception e) {
    final s = e.toString();
    final colon = s.indexOf(': ');
    return colon < 0 ? s : s.substring(colon + 2);
  }
}
