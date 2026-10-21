/// The queue's [AgentPort] over lib/agent's parts: a [TurnBackend] asks
/// the model one turn at a time, and a job's [Toolbox] runs its tool calls
/// (spec 2026-10-19-in-process-core-design §7.1: "the agent's runners and
/// backends, unchanged in substance").
library;

import 'dart:async';
import 'dart:convert';

import '../agent/backend.dart' show AgentError;
import '../agent/edit_result.dart' show DbOp, maxDbOps;
import '../agent/failure.dart'
    show
        LimitProgress,
        limitLandedReply,
        minKeepGoingUsd,
        planAfterStops;
import '../agent/icon_svg.dart' show appIconPath;
import '../agent/prompt.dart' show KeepGoingNote;
import '../agent/request.dart' show RequestKind;
import '../agent/runner.dart' show RunInput;
import '../agent/runner/messages_runner.dart' show finishNudge;
import '../agent/runner/turn.dart';
import '../agent/tools/ask_tool.dart' show AskTool;
import '../agent/tools/budget.dart';
import '../agent/tools/draft.dart';
import '../agent/tools/finish.dart'
    show FinishTool, GiveUpTool, iconAsk, stoppedEarlyReply, titleFromRequest;
import '../agent/tools/render_tool.dart' show RenderTool;
import '../agent/tools/tool_spec.dart';
import '../agent/tools/toolbox.dart';
import '../agent/unfinished.dart' show changedFiles, keptEdits;
import '../bundle/content_hash.dart' show contentHash, sha256Hex;
import 'model.dart';
import 'runner.dart' show AgentPort, PublishRefused;

/// One job's agent in the worker: the prompt its first run starts from,
/// the tools its calls run on over the job's draft, and what the opener
/// read of the job.
final class AgentJob {
  AgentJob({
    required this.input,
    required this.toolbox,
    this.row,
    this.capUsd,
    this.again,
    this.carried = const {},
  });

  /// The first run's prompt.
  final RunInput input;

  /// The first run's tools, over a draft of the job's app at the job's
  /// base, with the first run's budget.
  final Toolbox toolbox;

  /// The job's row as the opener read it. Its committed turns are what
  /// a worker that opens the job late (after a restart) rebuilds the draft,
  /// the history and the budget from; null for none.
  final Job? row;

  /// What all the job's runs may spend together (`buildCapUsd` for a
  /// make, `changeCapUsd` for a change): a stop at a limit offers Keep
  /// going only while at least `minKeepGoingUsd` of it is left. Null: no
  /// Keep going.
  final double? capUsd;

  /// How a run after a stop at a limit starts; null: it goes on with the
  /// first run's prompt and tools.
  final KeepGoingRun? again;

  /// The files of the job's base version the draft does not hold (those
  /// that are not text), by their hash and size: a publish of the draft
  /// carries them into its version, so its render digest counts them
  /// ([draftDigest]). None for a make.
  final Map<String, FileHash> carried;
}

/// A file's hex SHA-256 and size in bytes.
typedef FileHash = ({String sha256, int size});

/// A Keep going run of a job: a fresh conversation over the job's draft,
/// with a budget of its own.
final class KeepGoingRun {
  const KeepGoingRun({required this.first, required this.toolbox});

  /// The run's first message: [note] says what the runs before it left,
  /// [files] are the draft's files as the run starts.
  final List<Map<String, Object?>> Function(
    KeepGoingNote note,
    Map<String, String> files,
  )
  first;

  /// The run's tools over the job's [draft], with [budget].
  final Toolbox Function(Draft draft, RunBudget budget) toolbox;
}

/// Opens job [jobId]'s agent: its prompt and its [Toolbox] over a draft of
/// the job's app at the job's base version (the opener reads the job from
/// the core, and gives its row).
typedef OpenJob = Future<AgentJob> Function(JobId jobId);

/// Serves [snapshot] of job [jobId]'s draft as a draft artifact the UI
/// isolate can load (`POST /api/drafts`, then a publish of what changed):
/// its ID.
typedef ServeDraft =
    Future<String> Function(JobId jobId, DraftSnapshot snapshot);

/// The digest a render of [files] is recorded under (spec §8.2,
/// `clax_core::digest::manifest_digest`): the content hash of the files
/// the version a publish of them makes holds, but `icon.svg`. Those are
/// [files], and the base's files the draft does not hold that the publish
/// carries ([carried]; a file of [files] replaces one of the same path).
String draftDigest(
  Map<String, String> files, {
  Map<String, FileHash> carried = const {},
}) => contentHash({
  for (final e in carried.entries)
    if (e.key != 'icon.svg') e.key: e.value,
  for (final e in files.entries)
    if (e.key != 'icon.svg')
      e.key: (
        sha256: sha256Hex(utf8.encode(e.value)),
        size: utf8.encode(e.value).length,
      ),
});

/// A render [AgentTurns.renderSpec] refuses: the run used its renders.
/// The runner answers the render as failed with [message].
final class RendersSpent implements Exception {
  const RendersSpent(this.message);
  final String message;
  @override
  String toString() => message;
}

/// A test file of a draft, as the inspect view takes it.
final _testFile = RegExp(r'^tests/[A-Za-z0-9_\-./]{1,200}\.test\.js$');

/// The tools the queue runs itself, for a job's [Toolbox]: their specs
/// for the model, and a refusal should the toolbox ever be asked to run
/// one. `render`, `ask`, `finish` and `give_up` are each the queue's
/// (a `Render`, an `Ask`, a `Publish` and a stop), never side effects of
/// a toolbox call.
List<Tool> queueTools() => [
  _HeldTool(RenderTool.toolSpec),
  _HeldTool(AskTool.queuedSpec),
  _HeldTool(FinishTool.toolSpec),
  _HeldTool(GiveUpTool.toolSpec),
];

final class _HeldTool implements Tool {
  _HeldTool(this.spec);
  @override
  final ToolSpec spec;
  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async =>
      ToolRefused('${spec.name} is the queue\'s to run.');
}

/// [AgentPort] over a [TurnBackend] and each job's [Toolbox].
///
/// **A turn** sends the run's history, as `MessagesApiRunner` does: the
/// run's first message, then each assistant turn as it came and the
/// results of its calls in one user message, in the calls' order (a call
/// without a result answers as an error), with the budget's soft note on
/// the last; a turn that called no tool is followed by [finishNudge]. The
/// turn's `tool_use` blocks become its calls:
///
/// - `render` → [RenderCall]; when the queue runs it, [renderSpec] serves
///   the draft as the calls before it left it ([ServeDraft]), with its
///   revision, digest and test files (refused past the run's renders),
///   and [rendered] records what it drew on the draft;
/// - `finish` → [FinishCall] at the turn's base, its arguments kept in the
///   plan; when the queue publishes it, [publishPlan] applies `finish`'s
///   rules (lib/agent/tools/finish.dart) and makes the publish step's
///   `{artifact_id, request: {title, capabilities, note, files,
///   addresses}, docs, draft: {id, digest}, session_id}`. A change's call
///   carries its reply (`{threads, body, resolve}`);
/// - `ask` → [AskCall] with the call's questions;
/// - `give_up` → no call: a give up out of room is a stop at the limit
///   most nearly spent, any other fails the turn with its reason;
/// - any other → [OtherCall], which [tool] runs on the job's toolbox.
///
/// A turn whose answer was cut off, or holds a tool call that is not JSON,
/// fails (`TurnFailed(other)`); a refusal too.
///
/// **Limits.** Before it asks the model, a turn checks the run's budget,
/// rebuilt from the job's committed turns (each turn's notes keep its
/// usage). At a hard cap it asks nothing: with a clean render of a draft
/// that differs from the app, it answers one [FinishCall] whose plan is
/// that render's snapshot, whole, with the stop's reply (`finish`'s
/// `capStop`); else a [TurnStop] carrying `{cap, turn, spent_usd,
/// notes}`, Keep going offered only while the job's [AgentJob.capUsd]
/// has `minKeepGoingUsd` left and a change has not planned the rest. A
/// turn of a job whose row carries such a stop, at or after its `turn`,
/// is a Keep going run ([AgentJob.again]): a fresh conversation over the
/// draft, its cost cap what is left, told to plan the rest from the
/// second stop or when the row says `planRest`.
///
/// **Rebuilt from the rows.** A job's draft, history and budget live in
/// this worker while it runs the job; a worker that opens a job late
/// (after a restart) runs the committed turns' tool calls again on a
/// fresh draft, in order, so the draft holds every earlier turn's writes,
/// and takes the history and the budget from the turns' notes.
///
/// Each turn's notes keep what the renders of the turn before drew, as
/// the runner recorded it in the core (`rendered`: one
/// `{turn, call, draft_id, digest, errors, failed_tests}` per render
/// call). A rebuild marks the draft rendered at each such call when the
/// draft it rebuilt there has the kept digest, so a finish or a cap stop
/// after a restart publishes what was rendered, naming the draft the
/// core recorded it under; the call's result in the history then holds
/// only the kept errors and failing tests. A render the notes do not
/// keep (its turn is the last committed, or the rebuilt draft differs),
/// and a refused finish, say their result was not kept, and the draft
/// counts as not rendered until it renders again.
final class AgentTurns implements AgentPort {
  AgentTurns({
    required this.backend,
    required this.open,
    required this.serveDraft,
    this.session,
    this.maxTokens = 32000,
  });

  final TurnBackend backend;
  final OpenJob open;
  final ServeDraft serveDraft;

  /// The worker's live session, named by each publish step
  /// (`session_id`): the step does for it what its own publish would, and
  /// a change's reply step is made from it. None when null.
  final Future<String> Function()? session;
  final int maxTokens;

  final Map<JobId, Future<_Job>> _jobs = {};
  final Map<JobId, _Job> _open = {};

  /// Forgets job [jobId]'s history and tools (it ended, or its work
  /// stopped).
  @override
  void forget(JobId jobId) {
    unawaited(_jobs.remove(jobId));
    _open.remove(jobId);
  }

  /// Job [jobId], opened once; an open that failed is not kept, so the
  /// job's next turn or call opens it again.
  Future<_Job> _jobOf(JobId jobId) {
    if (_jobs[jobId] case final held?) return held;
    late final Future<_Job> opening;
    opening = open(jobId).then(
      (a) {
        final j = _Job(a);
        _open[jobId] = j;
        return j;
      },
      onError: (Object e, StackTrace s) {
        if (identical(_jobs[jobId], opening)) _jobs.remove(jobId);
        Error.throwWithStackTrace(e, s);
      },
    );
    return _jobs[jobId] = opening;
  }

  /// Job [jobId] ready to run turn [turn]'s calls ([calls]) or to start
  /// turn [turn] (not [calls]): opened, and rebuilt from its row when this
  /// worker did not run the turn before.
  Future<_Job> _ready(
    JobId jobId,
    int turn, {
    required bool calls,
    Carry? carry,
  }) async {
    var job = await _jobOf(jobId);
    final fits = calls
        ? job.live == turn
        : job.live == turn - 1 || job.live == turn;
    if (!fits && job.live > 0) {
      // The job moved on outside this worker: open it again.
      forget(jobId);
      job = await _jobOf(jobId);
    }
    if (job.live == 0 && turn > 1) {
      await job.rebuild(
        through: turn - 1,
        runOf: turn,
        carry: carry ?? job.agent.row?.carry,
      );
    }
    if (calls) job.live = turn;
    return job;
  }

  @override
  Future<TurnOutcome> turn(
    JobId jobId,
    TurnInput input,
    Future<void> cancelled,
  ) async {
    final t = input.turn;
    final job = await _ready(jobId, t, calls: false, carry: input.carry);
    if (t > 1) {
      if (input.previous case final notes?) job.notes[t - 1] = notes;
      job.results[t - 1] = input.results;
    }
    // A Keep going run starts at the turn the stop named.
    final start = _runStart(input.carry, t);
    if (job.runStart != start) {
      job.beginRun(start, input.carry, planRest: input.planRest);
    }
    // The turn before, when another worker asked it: its words and its
    // spend, from its notes (its renders count as this worker runs them).
    if (input.previous case final before? when t > 1) {
      job.assistant[t - 1] ??= switch (before.data) {
        {'assistant': final List content} => [
          for (final b in content)
            if (b is Map) b.cast<String, Object?>(),
        ],
        _ => const [],
      };
      if (t - 1 >= job.runStart && job.counted.add(t - 1)) {
        final (u, attempts) = usageOf(before);
        job.state.budget.addTurn(u, attempts: attempts);
      }
    }
    final state = job.state;
    final budget = state.budget;
    if (budget.hardStop case final cap?) {
      await _named(job);
      return _capStop(job, input, cap);
    }
    final TurnRequest request = TurnRequest(
      model: budget.model,
      system: job.system,
      tools: [for (final s in job.toolbox.specs) s.toMessagesApi()],
      messages: List.unmodifiable(job.messages(t)),
      maxTokens: _maxTokens(budget),
      outputCap: budget.caps.outputTokens,
      cancelled: cancelled,
      outputBefore: budget.usage.output,
    );
    budget.resumeClock();
    final ModelTurn turn;
    try {
      turn = await backend.turn(request);
    } finally {
      budget.pauseClock();
    }
    budget.addTurn(turn.usage, attempts: turn.attempts);
    if (turn.stopReason == 'refusal') {
      final category = turn.stopDetails?['category'] ?? 'no category given';
      throw AgentError('The model declined: $category');
    }
    if (turn.broken || turn.stopReason == 'max_tokens') {
      throw const AgentError('the answer was cut off', retryable: true);
    }
    final uses = [
      for (final b in turn.content)
        if (b['type'] == 'tool_use') b,
    ];
    final drawn = job.renderedIn(t - 1);
    final notes = <String, Object?>{
      'assistant': turn.content,
      'usage': _usageJson(turn.usage),
      if (turn.attempts.isNotEmpty)
        'attempts': [
          for (final a in turn.attempts)
            {'model': a.model, 'usage': _usageJson(a.usage)},
        ],
      if (drawn.isNotEmpty) _renderedNote: drawn,
    };
    job.assistant[t] = turn.content;
    job.counted.add(t);
    job.live = t;
    for (final b in uses) {
      if (b['name'] != 'give_up') continue;
      final args = _args(b['input']);
      final reason = switch (args['reason']) {
        final String r when r.trim().isNotEmpty => r.trim(),
        _ => 'the agent gave up',
      };
      if (args['out_of_room'] == true) {
        await _named(job);
        return _capStop(job, input, budget.mostSpent, narration: reason);
      }
      throw AgentError(reason);
    }
    final change = input.input.threads.isNotEmpty;
    final calls = <ToolCall>[
      for (final b in uses)
        _call(input, change, '${b['id']}', '${b['name']}', _args(b['input'])),
    ];
    return TurnOutcome(TurnNotes(calls: calls, data: notes));
  }

  /// Names the worker's session in [job]'s publish steps, as it is now
  /// (a session that ended is registered again); a session that cannot be
  /// had leaves them naming the one they named before, or none.
  Future<void> _named(_Job job) async {
    try {
      job.sessionId = await session?.call();
    } on Exception catch (_) {
      // The step publishes without one; a change's reply is then refused.
    }
  }

  /// The first turn of the run turn [t] belongs to: the turn a stop at a
  /// limit named, when the row carries one at or before [t]; else 1.
  static int _runStart(Carry? carry, int t) => switch (carry?.data) {
    {'turn': final int s} when s <= t && s > 1 => s,
    _ => 1,
  };

  int _maxTokens(RunBudget b) {
    final left = b.caps.outputTokens - b.usage.output;
    return left.clamp(1, maxTokens);
  }

  static Map<String, Object?> _args(Object? input) => switch (input) {
    final Map<String, Object?> m => m,
    final Map m => m.cast<String, Object?>(),
    _ => const <String, Object?>{},
  };

  static ToolCall _call(
    TurnInput input,
    bool change,
    String id,
    String name,
    Map<String, Object?> args,
  ) => switch (name) {
    // The draft is served when the queue runs the call, after the calls
    // before it: the spec here names nothing yet.
    'render' => RenderCall(id, const RenderSpec(draftId: '', revision: 0)),
    'finish' => FinishCall(
      id,
      PublishPlan(base: input.base, data: {'finish': args}),
      reply: change
          ? ReplyPlan(
              data: {
                'threads': [...input.input.threads.keys],
                'body': switch (args['reply']) {
                  final String r => r.trim(),
                  _ => '',
                },
                'resolve': args['resolve'] == true,
              },
            )
          : null,
    ),
    'ask' => AskCall(id, Question(data: {'questions': args['questions']})),
    _ => OtherCall(id, name, args: args),
  };

  /// The stop at the hard cap [cap] before turn [input]: publishes the
  /// last clean render's snapshot when it holds work, else stops.
  TurnOutcome _capStop(
    _Job job,
    TurnInput input,
    String cap, {
    String? narration,
  }) {
    final state = job.state;
    final d = state.draft;
    final make = state.ctx.kind == RequestKind.newApp;
    bool usable(DraftSnapshot? c) =>
        c != null &&
        c.files.containsKey('index.html') &&
        (make ||
            c.changedFrom(
              state.ctx.files,
              title: state.ctx.title,
              caps: state.ctx.capabilities,
            )) &&
        job.servedAt(c) != null;
    final passed = d.cleanOnBase;
    final clean = usable(passed) ? passed : d.pageCleanOnBase;
    final spent = job.spentBefore + state.budget.costUsd;
    final capUsd = job.agent.capUsd;
    final left = (capUsd ?? 0) - spent;
    final said = narration ?? job.narration;
    final notes = [...job.carriedNotes, ...job.runNotes(), ?narration];
    final drawn = job.renderedIn(input.turn - 1);
    if (usable(clean)) {
      final checksFailed = !identical(clean, passed);
      final reply = make
          ? '${stoppedEarlyReply(said, cap)}'
                '${checksFailed ? ' Some of its checks failed.' : ''}'
          : limitLandedReply(
              cap,
              leftUsd: left,
              checksFailed: checksFailed,
              progress: LimitProgress(
                stops: job.stops + 1,
                spentUsd: spent,
                notes: notes,
                changed: changedFiles(
                  keptEdits(clean!.files, state.ctx.files),
                  state.ctx.files,
                ),
              ),
            );
      final served = job.servedAt(clean!)!;
      final plan = job.planOf(
        clean.files,
        title: clean.title,
        capabilities: clean.capabilities,
        stagedDb: clean.stagedDb,
        pins: clean.pins,
        reply: reply,
        served: served,
        threads: [...input.input.threads.keys],
        fallbackTitle: true,
      );
      job.live = input.turn;
      return TurnOutcome(
        TurnNotes(
          calls: [
            FinishCall(
              _capStopCall,
              PublishPlan(base: input.base, data: plan),
              reply: make
                  ? null
                  : ReplyPlan(
                      data: {
                        'threads': [...input.input.threads.keys],
                        'body': reply,
                        'resolve': false,
                      },
                    ),
            ),
          ],
          data: {
            'assistant': const <Object?>[],
            'usage': _usageJson(const Usage()),
            'cap': cap,
            if (drawn.isNotEmpty) _renderedNote: drawn,
          },
        ),
      );
    }
    final canKeepGoing =
        capUsd != null &&
        left >= minKeepGoingUsd &&
        !(state.ctx.kind == RequestKind.change && state.planned);
    return TurnOutcome(
      const TurnNotes(),
      stop: TurnStop(
        canKeepGoing: canKeepGoing,
        data: {
          'cap': cap,
          'turn': input.turn,
          'spent_usd': spent,
          'notes': notes.length > _keptNotes
              ? notes.sublist(notes.length - _keptNotes)
              : notes,
        },
      ),
    );
  }

  /// The key of a turn's notes that keeps what the renders of the turn
  /// before drew.
  static const _renderedNote = 'rendered';

  /// The ID of the finish call a cap stop answers.
  static const _capStopCall = 'cap_stop';

  /// At most this many of the agent's notes are carried past a stop.
  static const _keptNotes = 12;

  @override
  Future<RenderSpec> renderSpec(
    JobId jobId,
    int turn,
    String callId,
    RenderSpec planned,
  ) async {
    final job = await _ready(jobId, turn, calls: true);
    final budget = job.state.budget;
    if (!budget.canRender) {
      throw RendersSpent(
        'this run has used its ${budget.caps.renders} renders; finish with '
        'what works',
      );
    }
    budget.addRender();
    final snap = job.state.draft.snapshot();
    final digest = draftDigest(snap.files, carried: job.agent.carried);
    final draftId = await serveDraft(jobId, snap);
    job.served['$turn/$callId'] = (
      draftId: draftId,
      digest: digest,
      snapshot: snap,
    );
    return RenderSpec(
      draftId: draftId,
      revision: snap.revision,
      kind: planned.kind,
      digest: digest,
      files: [...snap.files.keys.where(_testFile.hasMatch)]..sort(),
      migration: planned.migration,
    );
  }

  @override
  void rendered(JobId jobId, int turn, String callId, RenderOutcome outcome) {
    final job = _open[jobId];
    final served = job?.served['$turn/$callId'];
    if (job == null || served == null) return;
    job.mark(
      '$turn/$callId',
      served.snapshot,
      errors: outcome.errors,
      failedTests: outcome.failedTests,
    );
  }

  @override
  Future<PublishPlan> publishPlan(
    JobId jobId,
    int turn,
    String callId,
    PublishPlan planned,
  ) async {
    final data = planned.data;
    // A cap stop's plan was made whole at its turn, from the snapshot.
    if (data is Map && data['request'] != null) return planned;
    final job = await _ready(jobId, turn, calls: true);
    await _named(job);
    final args = switch (data) {
      {'finish': final Map a} => a.cast<String, Object?>(),
      _ => const <String, Object?>{},
    };
    return PublishPlan(base: planned.base, data: await job.finish(args));
  }

  @override
  Future<ToolResult> tool(
    JobId jobId,
    int turn,
    OtherCall call,
    Future<void> cancelled,
  ) async {
    final job = await _ready(jobId, turn, calls: true);
    return _resultOf(await job.toolbox.call(call.name, _args(call.args)));
  }

  static ToolResult _resultOf(ToolOutput out) => switch (out) {
    ToolOk(:final json, :final png) => ToolResult(
      data: {...json, if (png != null) 'png': base64Encode(png)},
    ),
    ToolRefused(:final message) => ToolResult(
      data: {'error': message},
      isError: true,
    ),
  };

  static Map<String, Object?> _usageJson(Usage u) => {
    'input': u.input,
    'cache_write': u.cacheWrite,
    'cache_read': u.cacheRead,
    'output': u.output,
  };

  static Usage _usageOf(Object? j) => switch (j) {
    final Map m => Usage(
      input: (m['input'] as num?)?.toInt() ?? 0,
      cacheWrite: (m['cache_write'] as num?)?.toInt() ?? 0,
      cacheRead: (m['cache_read'] as num?)?.toInt() ?? 0,
      output: (m['output'] as num?)?.toInt() ?? 0,
    ),
    _ => const Usage(),
  };

  /// Turn [notes]' usage and its attempts, as [turn] wrote them.
  static (Usage, List<TurnAttempt>) usageOf(TurnNotes notes) {
    final d = notes.data;
    if (d is! Map) return (const Usage(), const []);
    return (
      _usageOf(d['usage']),
      [
        for (final a in switch (d['attempts']) {
          final List l => l,
          _ => const [],
        })
          if (a case {'model': final String model})
            (model: model, usage: _usageOf(a['usage'])),
      ],
    );
  }
}

/// What a render served: the draft artifact, its digest and the snapshot.
typedef _Served = ({String draftId, String digest, DraftSnapshot snapshot});

/// One job's agent: its draft and tools, the current run's history and
/// budget, and what its renders served.
final class _Job {
  _Job(this.agent) : toolbox = agent.toolbox, first = agent.input.first {
    agent.toolbox.state.budget.pauseClock();
  }

  final AgentJob agent;

  /// The current run's tools; their state's draft is the job's.
  Toolbox toolbox;
  RunState get state => toolbox.state;
  String get system => agent.input.system;

  /// The current run's first message, and its first turn.
  List<Map<String, Object?>> first;
  int runStart = 1;

  /// The turn whose calls this worker ran last, or which it last asked
  /// the model; 0 before any.
  int live = 0;

  /// The turns whose usage the current run's budget counts.
  final Set<int> counted = {};

  /// The stops at a limit before the current run, what the runs before it
  /// spent, and their notes (the row's carry when the run began).
  int stops = 0;
  double spentBefore = 0;
  List<String> carriedNotes = const [];

  /// Each turn's assistant content, calls and results.
  final Map<int, List<Map<String, Object?>>> assistant = {};
  final Map<int, TurnNotes> notes = {};
  final Map<int, Map<String, ToolResult>> results = {};

  /// What each render served, by `<turn>/<call ID>`.
  final Map<String, _Served> served = {};

  /// What each render drew, as the runner recorded it, by
  /// `<turn>/<call ID>`: its uncaught errors and failing tests.
  final Map<String, ({List<String> errors, List<String> failedTests})> drawn =
      {};

  /// Records that render `<turn>/<call ID>` [key] drew [snapshot] with
  /// [errors] and [failedTests], and marks the draft rendered.
  void mark(
    String key,
    DraftSnapshot snapshot, {
    required List<String> errors,
    required List<String> failedTests,
  }) {
    drawn[key] = (errors: errors, failedTests: failedTests);
    state.draft.markRendered(
      snapshot,
      errors: errors.isNotEmpty,
      testsFailed: failedTests.isNotEmpty,
      problems: [...errors, for (final f in failedTests) 'test failed: $f'],
    );
  }

  /// What the renders of turn [t] served and drew, as the next turn's
  /// notes keep it: one entry per render call this worker saw drawn.
  List<Map<String, Object?>> renderedIn(int t) => [
    for (final c in notes[t]?.calls ?? const <ToolCall>[])
      if (c is RenderCall)
        if ((served['$t/${c.id}'], drawn['$t/${c.id}'])
            case (final s?, final d?))
          {
            'turn': t,
            'call': c.id,
            'draft_id': s.draftId,
            'digest': s.digest,
            'errors': d.errors,
            'failed_tests': d.failedTests,
          },
  ];

  /// Render call [callId] of turn [turn] drawn again from the rows: the
  /// entry turn [turn] + 1's notes keep of it, when the draft as rebuilt
  /// here has its digest. The draft is marked rendered as the render
  /// drew it; null when the rows keep no such entry.
  ToolResult? redrawn(int turn, String callId) {
    final next = agent.row?.steps[TurnKey(turn + 1)];
    if (next is! TurnStepResult) return null;
    final kept = switch (next.notes.data) {
      {AgentTurns._renderedNote: final List l} => l
          .whereType<Map>()
          .where((e) => e['turn'] == turn && e['call'] == callId)
          .firstOrNull,
      _ => null,
    };
    if (kept case {
      'draft_id': final String draftId,
      'digest': final String digest,
      'errors': final List errors,
      'failed_tests': final List failed,
    }) {
      final snap = state.draft.snapshot();
      if (draftDigest(snap.files, carried: agent.carried) != digest) {
        return null;
      }
      final key = '$turn/$callId';
      served[key] = (draftId: draftId, digest: digest, snapshot: snap);
      final e = [for (final x in errors) '$x'];
      final f = [for (final x in failed) '$x'];
      mark(key, snap, errors: e, failedTests: f);
      return ToolResult(
        data: {
          'note':
              'This render ran before Clax restarted; only its errors and '
              'failing tests were kept.',
          'errors': e,
          'failed_tests': f,
        },
        isError: e.isNotEmpty || f.isNotEmpty,
      );
    }
    return null;
  }

  /// Whether `finish` asked for the icon once.
  var askedIcon = false;

  /// The render that served [s], or one that served the same files.
  _Served? servedAt(DraftSnapshot s) {
    for (final v in served.values) {
      if (identical(v.snapshot, s)) return v;
    }
    return null;
  }

  /// Rebuilds the job from its row: runs the calls of turns 1 to
  /// [through] again on the draft, in order, and makes the run turn
  /// [runOf] belongs to ([carry] names a Keep going run's first turn): its
  /// first message from the draft as it began, its history and its budget
  /// from the turns' notes.
  Future<void> rebuild({
    required int through,
    required int runOf,
    Carry? carry,
  }) async {
    final row = agent.row;
    final planRest = row?.plan?.planRest ?? false;
    final start = AgentTurns._runStart(carry, runOf);
    for (var j = 1; j <= through; j++) {
      if (j == start && start > 1) beginRun(start, carry, planRest: planRest);
      final step = row?.steps[TurnKey(j)];
      if (step is! TurnStepResult) continue;
      final n = notes[j] = step.notes;
      if (n.data case {'assistant': final List content}) {
        assistant[j] = [
          for (final b in content)
            if (b is Map) b.cast<String, Object?>(),
        ];
      }
      final out = <String, ToolResult>{};
      for (final c in n.calls) {
        out[c.id] = switch (c) {
          OtherCall(:final name, :final args) => AgentTurns._resultOf(
            await toolbox.call(name, AgentTurns._args(args)),
          ),
          RenderCall() => redrawn(j, c.id) ?? _notKept('render'),
          AskCall() => switch (row?.questionOf(j, c.id)) {
            final q? => ToolResult.answer(q.answer),
            null => _notKept('ask'),
          },
          FinishCall() => ToolResult.refused(
            'not_kept',
            'This finish was refused before Clax restarted; its answer was '
                'not kept. Render the draft, then finish again.',
          ),
        };
      }
      results[j] = out;
      if (j >= start) {
        final (u, attempts) = AgentTurns.usageOf(n);
        state.budget.addTurn(u, attempts: attempts);
        for (final c in n.calls) {
          if (c is RenderCall) state.budget.addRender();
        }
        counted.add(j);
      }
    }
    if (start == runOf && start > 1) {
      beginRun(start, carry, planRest: planRest);
    }
    live = through;
  }

  static ToolResult _notKept(String tool) => ToolResult(
    data: {
      'note':
          'This $tool ran before Clax restarted; its result was not kept.'
          '${tool == 'render' ? ' Render again to see the draft.' : ''}',
    },
  );

  /// Begins the run whose first turn is [start]: the first run, or a Keep
  /// going run after the stop [carry] holds, its budget what the job has
  /// left and its first message what the runs before it left.
  void beginRun(int start, Carry? carry, {required bool planRest}) {
    runStart = start;
    counted.clear();
    if (start == 1 || carry == null) {
      toolbox = agent.toolbox;
      first = agent.input.first;
      return;
    }
    final data = switch (carry.data) {
      final Map d => d,
      _ => const {},
    };
    stops = carry.stops;
    spentBefore = switch (data['spent_usd']) {
      final num n => n.toDouble(),
      _ => 0,
    };
    carriedNotes = [
      if (data['notes'] case final List l)
        for (final n in l)
          if (n is String) n,
    ];
    final base = agent.toolbox.state.budget;
    final capUsd = agent.capUsd;
    final left = capUsd == null ? base.caps.costUsd : capUsd - spentBefore;
    final caps = RunCaps(
      turns: base.caps.turns,
      renders: base.caps.renders,
      outputTokens: base.caps.outputTokens,
      costUsd: left < base.caps.costUsd ? left : base.caps.costUsd,
      wallClock: base.caps.wallClock,
    );
    final draft = state.draft;
    final again = agent.again;
    if (again == null) return;
    final budget = RunBudget(caps, base.model, base.clock)..pauseClock();
    toolbox = again.toolbox(draft, budget);
    final ctx = toolbox.state.ctx;
    final edits = keptEdits(draft.files, ctx.files);
    first = again.first(
      KeepGoingNote(
        limit: switch (data['cap']) {
          final String c => c,
          _ => '',
        },
        stops: planRest && stops < planAfterStops ? planAfterStops : stops,
        notes: carriedNotes,
        edited: edits.keys.toList()..sort(),
      ),
      Map.of(draft.files),
    );
  }

  /// The history turn [t] sends: the run's first message, then each turn
  /// of the run before [t] with its calls' results.
  List<Map<String, Object?>> messages(int t) {
    final out = <Map<String, Object?>>[
      {'role': 'user', 'content': first},
    ];
    for (var j = runStart; j < t; j++) {
      final content = assistant[j] ?? const [];
      if (content.isEmpty) continue;
      out.add({'role': 'assistant', 'content': content});
      final calls = notes[j]?.calls ?? const <ToolCall>[];
      final soft = j == t - 1 ? state.budget.softNote : null;
      out.add({
        'role': 'user',
        'content': [
          if (calls.isEmpty) {'type': 'text', 'text': finishNudge},
          for (final c in calls) _result(c.id, results[j]?[c.id]),
          if (soft != null) {'type': 'text', 'text': soft},
        ],
      });
    }
    return out;
  }

  /// The first sentence of each text the current run's turns said, oldest
  /// first.
  List<String> runNotes() => [
    for (var j = runStart; j <= live; j++)
      for (final b in assistant[j] ?? const <Map<String, Object?>>[])
        if (b case {'type': 'text', 'text': final String text})
          ?_sentence(text),
  ];

  /// The latest note of the current run.
  String? get narration => runNotes().lastOrNull;

  static String? _sentence(String text) {
    final t = text.trim().split('\n').first.trim();
    if (t.isEmpty) return null;
    final stop = RegExp(r'[.!?…](\s|$)').firstMatch(t);
    final s = stop == null ? t : t.substring(0, stop.start + 1);
    return s.length > narrationMax ? '${s.substring(0, narrationMax)}…' : s;
  }

  /// The publish step a `finish` called with [args] makes of the draft
  /// now, under `finish`'s rules (lib/agent/tools/finish.dart): a reply,
  /// a new app's `index.html` and title, a changed draft rendered at its
  /// latest revision without uncaught errors or failing tests, the icon
  /// asked for once, and at most `maxDbOps` staged document writes. A rule
  /// broken is a [PublishRefused] the model reads.
  Future<Map<String, Object?>> finish(Map<String, Object?> args) async {
    final s = state;
    final d = s.draft;
    final make = s.ctx.kind == RequestKind.newApp;
    for (final k in args.keys) {
      if (k != 'reply' && k != 'resolve') {
        throw PublishRefused(
          'invalid_args',
          'finish takes no "$k"; it takes reply, resolve.',
        );
      }
    }
    final reply = switch (args['reply']) {
      final String r when r.trim().isNotEmpty => r.trim(),
      _ => throw const PublishRefused(
        'invalid_args',
        'reply must be a non-empty string.',
      ),
    };
    if (make) {
      if (!d.files.containsKey('index.html')) {
        throw const PublishRefused(
          'invalid_args',
          'A new app needs index.html: write it, render, then finish.',
        );
      }
      if (d.title?.trim().isEmpty ?? true) {
        throw const PublishRefused(
          'invalid_args',
          'A new app needs a title: set it with set_meta, then finish.',
        );
      }
    }
    final changed =
        make ||
        d.changedFrom(s.ctx.files, title: s.ctx.title, caps: s.ctx.capabilities);
    _Served? shown;
    if (changed) {
      if (d.renderedPageRevision == null) {
        throw const PublishRefused(
          'not_rendered',
          'Render the draft before finish: every change is checked before '
              'it is published.',
        );
      }
      if (d.renderedPageRevision != d.pageRevision) {
        throw const PublishRefused(
          'not_rendered',
          'The draft changed after its last render; render it again before '
              'finish.',
        );
      }
      if (d.lastRenderHadErrors) {
        throw const PublishRefused(
          'render_errors',
          'The last render had uncaught errors; fix them and render again, '
              'or call give_up.',
        );
      }
      if (d.lastRenderFailedTests) {
        throw const PublishRefused(
          'tests_failed',
          'The last render had failing tests; fix the app (or a test that '
              'is wrong) and render again, or call give_up.',
        );
      }
      for (final v in served.values) {
        if (v.snapshot.pageRevision == d.renderedPageRevision &&
            v.snapshot.revision == d.renderedRevision) {
          shown = v;
        }
      }
    }
    if (s.ctx.asksIcon && !askedIcon && !d.files.containsKey(appIconPath)) {
      askedIcon = true;
      throw const PublishRefused('icon', iconAsk);
    }
    if (d.stagedDb.length > maxDbOps) {
      throw PublishRefused(
        'invalid_args',
        'The draft stages ${d.stagedDb.length} document writes; at most '
            '$maxDbOps fit in one batch.',
      );
    }
    return planOf(
      d.files,
      title: d.title,
      capabilities: d.capabilities,
      stagedDb: d.stagedDb,
      pins: d.pins,
      reply: reply,
      served: shown,
      threads: [...s.ctx.threadIds],
    );
  }

  /// The publish step of [files], [title] and [capabilities] with the
  /// staged document writes [stagedDb] pinned by [pins], noted [reply]:
  /// a new app's every file, a change's files that differ from its base
  /// (a removed one as null) with `index.html`, its threads as the
  /// version's addresses, and the title and capabilities when they
  /// changed.
  Map<String, Object?> planOf(
    Map<String, String> files, {
    required String? title,
    required Map<String, Object?>? capabilities,
    required List<DbOp> stagedDb,
    required Map<String, int?> pins,
    required String reply,
    required _Served? served,
    required List<String> threads,
    bool fallbackTitle = false,
  }) {
    final ctx = state.ctx;
    final make = ctx.kind == RequestKind.newApp;
    final base = ctx.files;
    final Map<String, Object?> sent;
    if (make) {
      sent = {
        for (final e in files.entries) e.key: {'content': e.value},
      };
    } else {
      sent = {
        'index.html': {'content': files['index.html'] ?? base['index.html']},
        for (final e in files.entries)
          if (base[e.key] != e.value) e.key: {'content': e.value},
        for (final p in base.keys)
          if (!files.containsKey(p)) p: null,
      };
    }
    final named = switch (title?.trim()) {
      final t? when t.isNotEmpty => t,
      _ when make && fallbackTitle =>
        titleFromRequest(ctx.request) ?? 'New app',
      _ => null,
    };
    final sid = sessionId;
    return {
      'artifact_id': ctx.artifactId,
      'request': {
        if (make || named != ctx.title) 'title': ?named,
        if (capabilities != null &&
            (make || jsonEncode(capabilities) != jsonEncode(ctx.capabilities)))
          'capabilities': capabilities,
        'note': reply,
        'files': sent,
        if (!make && threads.isNotEmpty) 'addresses': threads,
      },
      if (stagedDb.isNotEmpty)
        'docs': [
          for (final o in stagedDb)
            {
              'op': o.op,
              'path': '${o.collection}/${o.docId}',
              if (o.op != 'delete') 'data': o.data ?? const <String, Object?>{},
              'if_version': ?pins['${o.collection}/${o.docId}'],
            },
        ],
      if (served != null)
        'draft': {'id': served.draftId, 'digest': served.digest},
      'session_id': ?sid,
    };
  }

  /// The session publish steps name, set by [AgentTurns] before a publish.
  String? sessionId;

  static Map<String, Object?> _result(String id, ToolResult? r) {
    if (r == null) {
      return {
        'type': 'tool_result',
        'tool_use_id': id,
        'is_error': true,
        'content': [
          {'type': 'text', 'text': 'The call has no result.'},
        ],
      };
    }
    final data = r.data;
    final png = _png(data);
    return {
      'type': 'tool_result',
      'tool_use_id': id,
      if (r.isError) 'is_error': true,
      'content': [
        {'type': 'text', 'text': jsonEncode(_withoutPng(data))},
        if (png != null)
          {
            'type': 'image',
            'source': {
              'type': 'base64',
              'media_type': 'image/png',
              'data': png,
            },
          },
      ],
    };
  }

  /// A screenshot a result carries: a tool's `png`, or a render's
  /// `inspection.png`.
  static String? _png(Object? data) => switch (data) {
    {'png': final String p} => p,
    {'inspection': {'png': final String p}} => p,
    _ => null,
  };

  static Object? _withoutPng(Object? data) => switch (data) {
    final Map m => {
      for (final e in m.entries)
        if (e.key != 'png') e.key: _withoutPng(e.value),
    },
    _ => data,
  };
}
