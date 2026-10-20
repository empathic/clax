import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import '../clax/api.dart';
import '../platform/clock.dart';
import 'apply.dart';
import 'asked.dart';
import 'backend.dart';
import 'edit_result.dart';
import 'failure.dart';
import 'icon_svg.dart' show appIconPath;
import 'inspector.dart';
import 'ledger.dart';
import 'line_diff.dart';
import 'plan.dart';
import 'progress.dart';
import 'prompt.dart';
import 'read_consent.dart';
import 'request.dart';
import 'runner.dart';
import 'timings.dart';
import 'tools/ask_tool.dart';
import 'tools/budget.dart';
import 'tools/data_tools.dart';
import 'tools/draft.dart';
import 'tools/file_tools.dart';
import 'tools/finish.dart';
import 'tools/kit_tool.dart';
import 'tools/render_tool.dart';
import 'tools/tool_spec.dart';
import 'tools/toolbox.dart';
import 'unfinished.dart';

/// The timing name of a new app's run.
const agentNewTiming = 'agent-new';

/// The timing name of a change's run.
const agentChangeTiming = 'agent-change';

/// The timing name of a build's continue run.
const agentContinueTiming = 'agent-continue';

/// The prefix of the key a first run keeps its plan under until its app
/// has an ID.
const pendingPlanPrefix = 'new:';

/// The single-call backends that send [AgentRequest.image] to the model.
/// `claude-cli` runs with tools off and cannot read an image. A tool-loop
/// runner says so itself ([AgentRunner.sendsImages]).
const imageBackends = {'messages-api', 'scripted'};

/// How [AgentLoop.change] ended.
sealed class ChangeOutcome {
  const ChangeOutcome();

  /// Whether every thread of the change that needed a reply got one.
  /// False when the daemon refused a reply or never received one, and for
  /// [ChangeInterrupted]; such a thread stays open with no agent comment
  /// after the person's, so the worker's recovery can run it again.
  bool get answered;
}

/// A new version landed.
final class ChangeLanded extends ChangeOutcome {
  const ChangeLanded(this.version, {this.answered = true});

  final int version;
  @override
  final bool answered;
}

/// The agent replied without changing a file (a question, or a change of
/// documents only).
final class ChangeAnswered extends ChangeOutcome {
  const ChangeAnswered({this.answered = true});

  @override
  final bool answered;
}

/// Nothing stayed published (a version whose documents were refused is
/// restored away); when [answered], each thread got a reply that opens
/// [failurePrefix] and names [reason], and stays open.
final class ChangeFailed extends ChangeOutcome {
  const ChangeFailed(this.reason, {required this.answered});

  final String reason;
  @override
  final bool answered;
}

/// The run was cancelled: nothing was published, replied or acknowledged.
final class ChangeCancelled extends ChangeOutcome {
  const ChangeCancelled();

  @override
  bool get answered => false;
}

/// The artifact is gone (404 `not_found`): the change was dropped without
/// a reply.
final class ChangeDropped extends ChangeOutcome {
  const ChangeDropped();

  @override
  bool get answered => false;
}

/// A daemon call got no answer ([ClaxApiError.unanswered]), so whether it
/// took effect is unknown. Nothing was replied or
/// acknowledged: the threads are left for the worker's recovery, which
/// runs them again, or replies naming the version when the publish did
/// land.
final class ChangeInterrupted extends ChangeOutcome {
  const ChangeInterrupted();

  @override
  bool get answered => false;
}

/// A new app's run stopped at a limit with work done and no app made
/// ([AgentLoop.newApp]): its draft is kept under [key] (the run's plan
/// key) in [AgentLoop.unfinished], for a Keep going
/// (`newApp(…, planKey: key, keepGoing: true)`) to run on from.
///
/// [stops] counts the make's runs that stopped, this one included;
/// [spentUsd] is what they cost together, of [capUsd] (the build's
/// [buildCapUsd], which the make's runs and its build share). [notes] are
/// the agent's words over those runs, oldest first, the last run's reason
/// for stopping among them ([reason], its own words); [changed] the files
/// the draft holds, with their lines; [html] the draft's `index.html`.
/// [planned]: the run that stopped had been told to plan the rest
/// ([planAfterStops]).
final class MakeStopped implements Exception {
  const MakeStopped({
    required this.key,
    required this.limit,
    required this.stops,
    required this.spentUsd,
    this.capUsd = buildCapUsd,
    this.notes = const [],
    this.changed = const [],
    this.reason,
    this.html,
    this.planned = false,
  });

  final String key;

  /// The cap the run stopped at, or gave up out of room near, as
  /// [RunBudget.hardStop] names it.
  final String limit;
  final int stops;
  final double spentUsd;
  final double capUsd;
  final List<String> notes;
  final List<FileChange> changed;
  final String? reason;
  final String? html;
  final bool planned;

  /// Whether enough of [capUsd] is left for another run.
  bool get canKeepGoing => capUsd - spentUsd >= minKeepGoingUsd;

  /// Whether the next Keep going is told to plan the rest.
  bool get plansNext => stops >= planAfterStops;

  @override
  String toString() => 'MakeStopped($key, $limit, stops: $stops)';
}

typedef _Snapshot = ({
  ArtifactSummary artifact,
  Map<String, String> files,
  List<AppCollection> docs,
});

/// One tool-loop run as it ended: its [outcome], whether a tool ended it
/// (`finish`, `give_up` or the cap stop, which reply themselves) or the
/// runner did ([byTool] false: a refusal, a broken turn, a cancel between
/// turns, a model failure), the model failure itself, and its `ask` tool.
typedef _Ran = ({
  RunOutcome outcome,
  bool byTool,
  AgentError? error,
  AskTool ask,
  RunState state,
  List<String> narrations,
  FinishTool finish,
  GiveUpTool giveUp,
});

/// At most this many characters of a Keep going's kept edits go into its
/// first message as a diff.
const maxKeepGoingDiff = 8 * 1024;

/// Where a Keep going run starts: from the stopped run's draft, or, when
/// the app changed under it, over from the app.
enum _Resume { draft, changed }

/// Asked before a run on app [aid] sends anything to the model: a change's
/// ([build] false) or a build's continue run ([build] true). False ends the
/// run cancelled.
typedef RunGate = Future<bool> Function(String aid, {required bool build});

/// Makes new apps and changes them in answer to comments, over the
/// daemon's API.
///
/// Each run takes one of two ways, chosen when it starts:
///
/// - **The tool loop** (R§2, R§4): an [AgentRunner] from [runner] moves the
///   model's tool calls through a [Toolbox] holding the file and data
///   tools, `render` (the draft served as a hidden draft artifact and
///   inspected by [inspector] at [baseUrl]), `ask` (Clax's questions, kept
///   in [asked] for a change), `finish` and `give_up`. Every rule (caps,
///   the publish gate, the change ledger, pinned document writes) is the
///   Toolbox's; this loop builds the run, ends it and says how it went.
/// - **The single structured call** (R§4.3, the `claude -p` fallback):
///   when [singleCall] answers a backend for the run, today's one
///   [EditResult] call, validated and applied here.
///
/// The tool loop's every end, however it came: the draft artifact is
/// deleted ([RenderTool.cleanup]); a change whose runner ended it without
/// a reply (a refusal, a broken turn, a model failure) gets the failure
/// reply ([failureReply]) and its comments are acknowledged; the run's
/// question is withdrawn when the run gave up or its app is gone, and for
/// a new app whenever it is still open ([AskTool.withdrawOpen]). A change
/// that finished with its question open is parked: its [AskedEntry] stays
/// for a late answer.
///
/// Retries of the single call: a result that fails [validateResult], whose
/// edit misses ([EditMiss]), or that writes an existing document the
/// prompt did not show, is asked for once more, naming the failure and
/// asking for whole files; a publish conflict re-reads the latest version
/// and runs the change once more. Document writes follow `finish`'s rule
/// ([writeStagedDocs]): after a conflict a write the document already
/// holds is dropped, and a document someone else changed meanwhile is
/// never written over: the version before the change is restored and the
/// change fails. A backend [AgentError]
/// is never resent, whatever its `retryable` says: backends retry their
/// own transport failures.
///
/// [timings] receives one record per run ([agentNewTiming],
/// [agentChangeTiming]); a tool-loop record also carries the run's turns,
/// tokens, cost and outcome. Give it a store kept apart from the defended
/// numbers (`sayUsable`, `longpressLanded`), so their recent list holds
/// only those.
///
/// Other apps go into a first message by ID and title only: their
/// documents are read through the data tools, after the owner's own
/// consent ([readConsent]; without one such reads are refused).
///
/// A change and a build's continue run first ask [gate] (when given)
/// whether they may send anything to the model; one it refuses ends
/// [ChangeCancelled] having sent, marked and replied nothing. A new app's
/// caller asks before [newApp].
class AgentLoop {
  AgentLoop({
    required this._api,
    AgentRunner Function()? runner,
    required this._prompts,
    required this._model,
    required this._timings,
    this._inspector,
    this._baseUrl,
    ChangeLedger? ledger,
    AskedQuestions? asked,
    PlanStore? plans,
    UnfinishedChanges? unfinished,
    this._readConsent,
    AgentBackend? Function()? singleCall,
    this._clock = const SystemClock(),
    bool Function(AgentBackend backend)? sendsImage,
    this._onAsked,
    this._fromBundle,
    this._gate,
    void Function(String line)? log,
  }) : assert(
         runner != null || singleCall != null,
         'an AgentLoop needs a runner or a single call',
       ),
       ledger = ledger ?? MemoryChangeLedger(),
       asked = asked ?? MemoryAskedQuestions(),
       plans = plans ?? PlanStore(),
       unfinished = unfinished ?? MemoryUnfinishedChanges(),
       _runner = runner,
       _singleCall = singleCall,
       _sendsImage = sendsImage ?? ((b) => imageBackends.contains(b.name)),
       _log = log ?? ((_) {});

  /// What each change published, for [settle]; in memory unless given.
  final ChangeLedger ledger;

  /// What each change's still-open question was asked for, so a late
  /// answer runs that change again; in memory unless given.
  final AskedQuestions asked;

  /// Each app's build plan; in memory unless given.
  final PlanStore plans;

  /// The changes that stopped at a limit, for Keep going; in memory
  /// unless given.
  final UnfinishedChanges unfinished;

  final ClaxApi _api;
  final AgentRunner Function()? _runner;
  final PromptBuilder _prompts;
  final String Function() _model;
  final Timings _timings;
  final Inspector? _inspector;
  final Uri Function()? _baseUrl;
  final AgentReadConsent? _readConsent;
  final AgentBackend? Function()? _singleCall;
  final Clock _clock;
  final void Function(QuestionView q)? _onAsked;
  final bool Function(AgentBackend) _sendsImage;
  final FutureOr<bool> Function(String aid)? _fromBundle;
  final RunGate? _gate;
  final void Function(String) _log;

  /// Whether a run on app [aid] may start: [_gate]'s answer, or true.
  Future<bool> _mayRun(String aid, {required bool build}) async =>
      await _gate?.call(aid, build: build) ?? true;

  /// Whether app [aid]'s code came from someone else's bundle (threat T2
  /// of the bundles spec): its runs see no other apps and their data
  /// tools refuse to read them. A lookup that fails counts as true.
  Future<bool> _isFromBundle(String aid) async {
    final f = _fromBundle;
    if (f == null) return false;
    try {
      return await f(aid);
    } on Object catch (e) {
      _log('the from-bundle lookup of $aid failed: $e');
      return true;
    }
  }

  /// Makes a new app from the person's [request] and returns its artifact
  /// ID, as the app session [session] (which also asks [AskTool]'s
  /// question; without one the agent cannot ask). Throws [AgentError]: a
  /// model failure, a run that gave up (its reason), a refused create, or
  /// [AgentError.cancelled] when [cancelled] fires before the create.
  ///
  /// [onProgress] receives the run's [AgentProgress] events, from an
  /// [AttemptStarted]. Without it the single call does not stream. A
  /// tool-loop app made without its starting documents (the service
  /// refused their batch) is still returned, after a [StartingDataRefused].
  ///
  /// A tool-loop create that got no answer (the daemon went away) looks
  /// for the app among the artifacts that were not there before the run,
  /// made by the app session, and returns it when found; else it throws,
  /// saying to look for it before trying again, so a Retry does not make
  /// it twice.
  ///
  /// The tool loop's run is a build's **first** run ([RunCaps.first]): it
  /// plans with `plan_set`, kept in [plans] under [planKey] (one that
  /// starts with [pendingPlanPrefix]; a fresh one when null), so a screen
  /// can watch the plan before the app exists. Once the app is made, the
  /// plan moves to its ID. A run that made no app keeps the plan under a
  /// [planKey] it was given, so a retry with the same key starts from it
  /// (the first message shows it), and forgets one it made up. The run's
  /// cost is the build's first spend.
  ///
  /// Limits (the tool loop): a run that stops at a cap, or gives up out of
  /// room, with a draft that holds work and no app made throws
  /// [MakeStopped], keeping the draft in [unfinished] under the plan key
  /// (the plan stays there too): its files, title and capabilities, the
  /// agent's notes, what the person answered, how many times the make
  /// stopped and what its runs cost. [keepGoing] (with that [planKey]) runs
  /// on from it: the draft starts from the kept files, and the first
  /// message carries them, the notes and the count; from [planAfterStops]
  /// stops the run is told to plan what is left and finish what works, for
  /// the build to build the rest. The make's runs and its build share
  /// [buildCapUsd]: each run's cost cap is what is left of it, at most
  /// [RunCaps.first]'s, and a Keep going with too little left
  /// ([minKeepGoingUsd]) runs nothing and throws the kept [MakeStopped]
  /// again. A stop with a clean render makes the app from that render
  /// instead (as [FinishTool.capStop] does), and the rest of the draft is
  /// kept for the build's next run ([buildDraftKey]) when the plan has
  /// features to build. Once an app is made, the kept draft is forgotten.
  Future<String> newApp(
    String request, {
    Future<void>? cancelled,
    ProgressSink? onProgress,
    String? session,
    String? planKey,
    bool keepGoing = false,
  }) async {
    final single = _singleCall?.call();
    if (single != null) {
      return _newAppSingle(single, request, cancelled, onProgress);
    }
    final key =
        planKey ?? '$pendingPlanPrefix${_clock.now().microsecondsSinceEpoch}';
    String? made;
    var stopped = false;
    try {
      made = await _newAppTools(
        request,
        cancelled,
        onProgress,
        session,
        key,
        keepGoing: keepGoing,
      );
      return made;
    } on MakeStopped {
      stopped = true;
      rethrow;
    } finally {
      try {
        if (made != null) {
          await plans.move(key, made);
          await unfinished.remove([key]);
        } else if (planKey == null && !stopped) {
          await plans.remove(key);
        }
      } on Object catch (e) {
        _log('keeping the plan of $key failed: $e');
      }
    }
  }

  /// The [MakeStopped] of the make kept under [key].
  static MakeStopped _stoppedOf(
    String key,
    UnfinishedChange k, {
    String? reason,
    bool planned = false,
  }) => MakeStopped(
    key: key,
    limit: k.limit,
    stops: k.stops,
    spentUsd: k.spentUsd,
    notes: k.notes,
    changed: changedFiles(k.edits, const {}),
    reason: reason,
    html: k.edits['index.html'],
    planned: planned,
  );

  Future<String> _newAppTools(
    String request,
    Future<void>? cancelled,
    ProgressSink? onProgress,
    String? session,
    String planKey, {
    bool keepGoing = false,
  }) async {
    final cancel = CancelWatch(cancelled);
    UnfinishedChange? kept;
    if (keepGoing) {
      try {
        kept = await unfinished.get(planKey);
      } on Object catch (e) {
        _log('the stopped make $planKey is unavailable: $e');
      }
    }
    final spentBefore = kept?.spentUsd ?? 0;
    final stopsBefore = kept?.stops ?? 0;
    final left = buildCapUsd - spentBefore;
    if (kept != null && left < minKeepGoingUsd) throw _stoppedOf(planKey, kept);
    final start = kept?.edits ?? const <String, String?>{};
    final runner = _requireRunner();
    final Uri base;
    try {
      base = _baseUrl!();
    } on ClaxApiError catch (e) {
      throw AgentError(e.message);
    }
    final listing = _api.listArtifacts();
    final reads = await Future.wait<Object?>([
      listing.then<List<ArtifactSummary>?>(
        (l) => l,
        onError: (Object _) => null,
      ),
      _othersFrom(listing),
      _prompts.toolSystem(),
      _plan(planKey),
    ]);
    final before = switch (reads[0]) {
      final List<ArtifactSummary> l => {for (final a in l) a.id},
      _ => null,
    };
    final others = reads[1]! as List<OtherApp>;
    final system = reads[2]! as String;
    final earlier = reads[3] as Plan?;
    cancel.check();
    final scope = _RunCancel(cancelled);
    const caps = RunCaps.first;
    final planning = kept != null && stopsBefore >= planAfterStops;
    final ran = await _runTools(
      runner,
      base,
      scope,
      RunContext(
        kind: RequestKind.newApp,
        files: const {},
        sessionId: session,
        cancelled: scope.future,
        clock: _clock,
        request: request,
        asksIcon: true,
      ),
      left >= caps.costUsd
          ? caps
          : RunCaps(
              turns: caps.turns,
              renders: caps.renders,
              outputTokens: caps.outputTokens,
              costUsd: left,
              wallClock: caps.wallClock,
            ),
      RunInput(
        system: system,
        first: _prompts.firstNew(
          request,
          others,
          plan: earlier,
          keepGoing: kept == null
              ? (keepGoing
                    ? const KeepGoingNote(
                        limit: '',
                        stops: 0,
                        notes: [],
                        why: 'what that run left is no longer kept',
                      )
                    : null)
              : KeepGoingNote(
                  limit: kept.limit,
                  stops: kept.stops,
                  notes: kept.notes,
                  lastRender: kept.lastRender,
                  edited: kept.edits.keys.toList()..sort(),
                ),
          files: {for (final e in start.entries) e.key: ?e.value},
          answers: kept?.answers,
        ),
      ),
      onProgress,
      agentNewTiming,
      const {},
      kind: RunKind.first,
      plan: RunPlan(plans, planKey, request: request, seen: earlier?.revision),
      start: start,
      startTitle: kept?.title,
      startCapabilities: kept?.capabilities,
    );
    // A new app's question is about nothing that outlives the run.
    await ran.ask.withdrawOpen();
    if (ran.outcome case RunGaveUp(:final limit?, :final reason)) {
      final d = ran.state.draft;
      final edits = keptEdits(d.files, const {});
      if (edits.isNotEmpty) {
        final k = UnfinishedChange(
          artifactId: planKey,
          threadIds: [planKey],
          commentIds: const [],
          limit: limit,
          spentUsd: spentBefore + ran.state.budget.costUsd,
          at: _clock.now(),
          stops: stopsBefore + 1,
          notes: _newest([
            ...?kept?.notes,
            ...ran.narrations,
            ?ran.giveUp.givenReason,
          ], maxUnfinishedNotes),
          answers: _joined([
            ?kept?.answers,
            for (final q in ran.ask.answered) answerText(q),
          ]),
          lastRender: _lastRender(d),
          edits: edits,
          before: {for (final p in edits.keys) p: null},
          title: d.title,
          capabilities: d.capabilities,
        );
        await _quiet('keep the stopped make', () => unfinished.put(k));
        throw _stoppedOf(
          planKey,
          k,
          reason: ran.giveUp.givenReason ?? reason,
          planned: planning,
        );
      }
    }
    switch (ran.outcome) {
      case RunFinished(:final artifactId?, :final docsRefused):
        if (ran.finish.capPublished case final clean?) {
          await _keepForBuild(artifactId, planKey, clean.files, ran.state);
        }
        if (docsRefused != null) {
          _log('new app $artifactId: made without its documents: $docsRefused');
          try {
            onProgress?.call(StartingDataRefused(artifactId, docsRefused));
          } on Object catch (e) {
            _log('a progress listener failed: $e');
          }
        }
        return artifactId;
      case RunFinished():
        throw const AgentError('The app was not created.');
      case RunCancelled():
        throw const AgentError.cancelled();
      case RunGaveUp(:final reason):
        throw ran.error ?? AgentError(ran.giveUp.givenReason ?? reason);
      case RunDropped():
        throw const AgentError('The app was deleted before it was finished.');
      case RunInterrupted():
        if (await _findCreated(before) case final aid?) return aid;
        throw const AgentError(
          'Clax stopped while saving your app, so it may have been made. '
          'Look for it in your apps before you try again.',
        );
    }
  }

  /// After a stop that made app [aid] from a clean render of [published]
  /// files: keeps what the draft holds beyond them for the build's next
  /// continue run ([buildDraftKey]), when the plan under [planKey] has
  /// features to build.
  Future<void> _keepForBuild(
    String aid,
    String planKey,
    Map<String, String> published,
    RunState state,
  ) async {
    final d = state.draft;
    final edits = keptEdits(d.files, published);
    if (edits.isEmpty) return;
    final p = await _plan(planKey);
    if (p == null || p.todo.isEmpty) return;
    await _quiet(
      'keep the draft for the build',
      () => unfinished.put(
        UnfinishedChange(
          artifactId: aid,
          threadIds: [buildDraftKey(aid)],
          commentIds: const [],
          limit: '',
          spentUsd: 0,
          at: _clock.now(),
          edits: edits,
          before: {for (final p in edits.keys) p: fingerprint(published[p])},
          lastRender: _lastRender(d),
        ),
      ),
    );
  }

  /// The artifact the app session made during an interrupted create: one
  /// not in [before] whose version 1 the app session published; null when
  /// there is none, or the daemon cannot say.
  Future<String?> _findCreated(Set<String>? before) async {
    if (before == null) return null;
    try {
      for (final a in await _api.listArtifacts()) {
        if (before.contains(a.id) || a.kind != 'html') continue;
        final d = await _api.getArtifact(a.id);
        final first = d.versions.where((v) => v.n == 1).firstOrNull;
        if (first != null &&
            first.sessionId != null &&
            first.agentHarness == appHarness) {
          _log('new app ${a.id} was made before its answer was lost');
          return a.id;
        }
      }
    } on Object catch (e) {
      _log('looking for an interrupted new app failed: $e');
    }
    return null;
  }

  Future<String> _newAppSingle(
    AgentBackend backend,
    String request,
    Future<void>? cancelled,
    ProgressSink? onProgress,
  ) async {
    final cancel = CancelWatch(cancelled);
    final reads = await Future.wait<Object>([
      _othersFrom(_api.listArtifacts()),
      _prompts.system(),
    ]);
    final others = reads[0] as List<OtherApp>;
    final system = reads[1] as String;
    final base = _prompts.newApp(request, others);
    String? retryReason;
    var attempt = 0;
    while (true) {
      cancel.check();
      final text = retryReason == null
          ? base
          : '$base\n${_retryText(retryReason)}';
      final r = await _run(
        ++attempt,
        backend,
        RequestKind.newApp,
        system,
        text,
        null,
        cancelled,
        onProgress,
        const {},
      );
      cancel.check();
      final Map<String, String?> files;
      try {
        validateResult(r, kind: RequestKind.newApp, existingPaths: const {});
        files = applyResult(const {}, r);
      } on ResultInvalid catch (e) {
        if (retryReason != null) throw AgentError(e.reason);
        retryReason = e.reason;
        continue;
      } on EditMiss catch (e) {
        if (retryReason != null) throw AgentError(e.reason);
        retryReason = e.reason;
        continue;
      }
      final PublishResult created;
      try {
        created = await _api.createArtifact(
          PublishRequest(
            title: r.title!.trim(),
            capabilities: r.capabilities,
            note: r.reply,
            files: {
              for (final e in files.entries)
                if (e.value != null) e.key: e.value,
            },
          ),
        );
      } on ClaxApiError catch (e) {
        throw AgentError(e.message);
      }
      final aid = created.artifact.id;
      if (r.db.isNotEmpty) {
        String? refused;
        try {
          refused = await writeStagedDocs(_api, aid, r.db, const {});
        } on ClaxApiError catch (e) {
          refused = e.message;
        }
        if (refused != null) {
          _log('new app $aid: its starting documents were refused: $refused');
        }
      }
      return aid;
    }
  }

  /// Changes artifact [aid] in answer to [items] (comments, possibly
  /// several of one thread), and never throws: a failure becomes a reply on
  /// the threads ([failureReply]), which stay open.
  ///
  /// First, every [AskedEntry] whose comments overlap [items] is taken and
  /// its question withdrawn: this run answers those comments, so a late
  /// answer to the earlier run's question must not run them again.
  ///
  /// Then: mark working; read the current version's text files, the app's
  /// own documents (this app's only, the versions they were shown at are
  /// what a write may be pinned to), the other apps by ID and title, the
  /// clip (when the run sends images) and the system prompt, all at once;
  /// run; acknowledge the comments to [session] (skipped when null) as
  /// the run's end replies; clear the working mark.
  ///
  /// [answers] is the daemon's late-answer text for a question an earlier
  /// run of these comments asked; the first message carries it. The
  /// entries of [answering], the questions it answers, are left to the
  /// caller, which takes them once the run ended for good.
  ///
  /// A thread whose latest viewer comment is not one of [items] (the
  /// person wrote again while this ran) gets neither the reply nor the
  /// resolve: the newer comment's own run answers it, and until then it
  /// stays recoverable. A daemon call without an answer ends the run as
  /// [ChangeInterrupted]. When [cancelled] fires before the publish, the
  /// run stops with [ChangeCancelled] and nothing is published, replied or
  /// acknowledged. [onProgress] receives the run's progress, as for
  /// [newApp].
  ///
  /// Limits (the tool loop): a run that ends at or near a cap of
  /// [RunCaps.change] replies in Clax's words naming the limit, with the
  /// agent's words under it ([limitFailureReply], [limitLandedReply]), and
  /// is kept in [unfinished]: its comments, its unfinished draft as edits
  /// on the app, the agent's notes and what it cost. [keepGoing] runs such
  /// a change on: the draft starts from those edits, where the app still
  /// holds what they were made on, and the first message carries the
  /// notes and what the person answered of its questions; an app that
  /// changed under the draft starts over from the app
  /// with the notes, and the run is told so. The draft is kept until
  /// then, however long ago the run stopped. With nothing kept, a
  /// [keepGoing] starts over from the app too, told so; a run that starts
  /// over opens its replies with [startedOverLine]. The change and its Keep
  /// going runs share
  /// [changeCapUsd]: each run's cost cap is what is left of it, at most
  /// [RunCaps.change]'s. Any other end of a run on these threads forgets
  /// what was kept, as does a [keepGoing] whose comments are not the kept
  /// ones (the person wrote again).
  Future<ChangeOutcome> change(
    String aid,
    List<FeedbackItem> items, {
    String? session,
    Future<void>? cancelled,
    ProgressSink? onProgress,
    String? answers,
    Set<String> answering = const {},
    bool keepGoing = false,
  }) async {
    if (items.isEmpty) return const ChangeAnswered();
    if (!await _mayRun(aid, build: false)) return const ChangeCancelled();
    final threadIds = [
      ...{for (final i in items) i.threadId},
    ];
    final (:kept, :lost) = keepGoing
        ? await _unfinishedOf(aid, items)
        : (kept: null, lost: false);
    await _takeOverQuestions(items, session, keep: answering);
    await _quiet(
      'mark working',
      () => _api.setWorking(
        aid,
        on: true,
        note: kept == null && !lost
            ? _workingNote(items)
            : 'Keep going: ${_workingNote(items)}',
        threadIds: threadIds,
      ),
    );
    final ChangeOutcome outcome;
    try {
      final single = _singleCall?.call();
      outcome = single != null
          ? await _changeSingle(
              single,
              aid,
              items,
              threadIds,
              session,
              cancelled,
              onProgress,
            )
          : await _changeTools(
              aid,
              items,
              threadIds,
              session,
              cancelled,
              onProgress,
              answers,
              kept,
              lost: lost,
            );
    } finally {
      await _quiet('clear working', () => _api.setWorking(aid, on: false));
    }
    return outcome;
  }

  /// The stop count and spend the latest limit reply of this app on
  /// [threadIds] states ([parseLimitProgress]), or null.
  Future<({int stops, double spentUsd})?> _carriedFrom(
    String aid,
    List<String> threadIds,
  ) async {
    try {
      ({int stops, double spentUsd})? best;
      for (final t in await _api.threads(aid)) {
        if (!threadIds.contains(t.id)) continue;
        for (final c in t.comments.reversed) {
          if (!c.byAgent || c.viaHarness != appHarness) continue;
          if (parseLimitProgress(c.body) case final p?) {
            if (best == null || p.stops > best.stops) best = p;
            break;
          }
        }
      }
      return best;
    } on Object catch (e) {
      _log('the limit replies of $aid are unavailable: $e');
      return null;
    }
  }

  /// The [UnfinishedChange] kept for [items]' threads on [aid], when it
  /// answered every one of [items]; else null, with [lost] true when
  /// nothing is kept for them (or it cannot be read), false when what is
  /// kept answered other comments (the person wrote again).
  Future<({UnfinishedChange? kept, bool lost})> _unfinishedOf(
    String aid,
    List<FeedbackItem> items,
  ) async {
    try {
      final c = await unfinished.get(items.first.threadId);
      if (c == null || c.artifactId != aid) return (kept: null, lost: true);
      final ids = c.commentIds.toSet();
      return items.every((i) => ids.contains(i.commentId))
          ? (kept: c, lost: false)
          : (kept: null, lost: false);
    } on Object catch (e) {
      _log('the unfinished change of $aid is unavailable: $e');
      return (kept: null, lost: true);
    }
  }

  /// After a change run on [threadIds] of [aid]: keeps it in [unfinished]
  /// when it ended at a limit, else forgets what was kept for them. The
  /// notes kept are [earlierNotes] (those of the runs a Keep going went on
  /// from) then this run's, the newest [maxUnfinishedNotes].
  Future<void> _keepUnfinished(
    String aid,
    List<FeedbackItem> items,
    List<String> threadIds,
    Map<String, String> base,
    double spentBefore,
    _Ran ran, {
    List<String> earlierNotes = const [],
    String? earlierAnswers,
    int earlierStops = 0,
  }) async {
    final limit = switch (ran.outcome) {
      RunGaveUp(:final limit?) || RunFinished(:final limit?) => limit,
      RunCancelled() || RunInterrupted() => null,
      _ => '',
    };
    // A limit stop after planning the rest: the build goes on with it.
    final planned = switch (ran.outcome) {
      RunGaveUp(planned: true) => true,
      RunFinished(limit: null) =>
        ran.state.planned && ran.finish.capPublished != null,
      _ => false,
    };
    try {
      if (limit == null) return;
      final d = ran.state.draft;
      // What the draft holds beyond what is live now: the run's base, or
      // the part a cap stop published.
      final from = ran.finish.capPublished?.files ?? base;
      final edits = keptEdits(d.files, from);
      final title = d.title != ran.state.ctx.title ? d.title : null;
      final caps =
          jsonEncode(d.capabilities) != jsonEncode(ran.state.ctx.capabilities)
          ? d.capabilities
          : null;
      if (limit.isEmpty) {
        await unfinished.remove(threadIds);
        if (planned && (edits.isNotEmpty || title != null || caps != null)) {
          // Its draft goes to the build, whose next run starts from it.
          await unfinished.put(
            UnfinishedChange(
              artifactId: aid,
              threadIds: [buildDraftKey(aid)],
              commentIds: const [],
              limit: '',
              spentUsd: 0,
              at: _clock.now(),
              edits: edits,
              before: {for (final p in edits.keys) p: fingerprint(from[p])},
              title: title,
              capabilities: caps,
              lastRender: _lastRender(d),
            ),
          );
        }
        return;
      }
      await unfinished.put(
        UnfinishedChange(
          artifactId: aid,
          threadIds: threadIds,
          commentIds: [for (final i in items) i.commentId],
          limit: limit,
          spentUsd: spentBefore + ran.state.budget.costUsd,
          at: _clock.now(),
          stops: earlierStops + 1,
          notes: _newest([
            ...earlierNotes,
            ...ran.narrations,
            ?ran.giveUp.givenReason,
          ], maxUnfinishedNotes),
          answers: _joined([
            ?earlierAnswers,
            for (final q in ran.ask.answered) answerText(q),
          ]),
          lastRender: _lastRender(d),
          edits: edits,
          before: {for (final p in edits.keys) p: fingerprint(from[p])},
          title: title,
          capabilities: caps,
        ),
      );
    } on Object catch (e) {
      _log('keeping the unfinished change of $aid failed: $e');
    }
  }

  /// What [d]'s latest render found, in a sentence or two; null when the
  /// run did not render.
  static String? _lastRender(Draft d) {
    if (d.renderedRevision == null) return null;
    String cut(String s) => s.length <= 300 ? s : '${s.substring(0, 300)}…';
    final problems = d.lastRenderProblems;
    final found = problems.isEmpty
        ? 'no uncaught errors or failing tests'
        : [
            for (final p in problems.take(8)) cut(p),
            if (problems.length > 8) 'and ${problems.length - 8} more',
          ].join('; ');
    return d.renderedRevision == d.revision
        ? found
        : '$found (the draft was edited after it)';
  }

  /// The kept [edits] against [files], the app as it is now, as unified
  /// diffs, at most [maxKeepGoingDiff] characters.
  static String? _diffOf(
    Map<String, String?> edits,
    Map<String, String> files,
  ) {
    if (edits.isEmpty) return null;
    final b = StringBuffer();
    for (final p in edits.keys.toList()..sort()) {
      b.write(unifiedDiff(p, files[p], edits[p]));
    }
    final text = b.toString();
    return text.length <= maxKeepGoingDiff
        ? text
        : '${text.substring(0, maxKeepGoingDiff)}\n'
              '… (the diff goes on; the files above hold the edits whole)';
  }

  static String? _joined(List<String> texts) =>
      texts.isEmpty ? null : texts.join('\n\n');

  static List<String> _newest(List<String> notes, int n) =>
      notes.length <= n ? notes : notes.sublist(notes.length - n);

  /// Runs the next features of artifact [aid]'s plan: a build's
  /// **continue** run ([RunCaps.continueBuild], its cost cap no more than
  /// what is left of [buildCapUsd]). Never throws.
  ///
  /// The plan is read at the start, so the person's edits made since the
  /// last run count; with no plan, or nothing to build, it returns
  /// [ChangeAnswered] without a run. The first message holds the plan, the
  /// app's files (as a change's does) and its documents. The run marks
  /// features as it builds them, and publishes a version as a change does,
  /// with no thread to answer; the working mark carries the next feature's
  /// title. A run that ends without publishing is [ChangeFailed] (its
  /// [ChangeFailed.answered] true: there is no thread to reply on).
  Future<ChangeOutcome> continueBuild(
    String aid, {
    Future<void>? cancelled,
    ProgressSink? onProgress,
  }) async {
    if (!await _mayRun(aid, build: true)) return const ChangeCancelled();
    final cancel = CancelWatch(cancelled);
    final Plan? plan;
    try {
      plan = await plans.get(aid);
    } on Object catch (e) {
      return ChangeFailed('its plan could not be read: $e', answered: true);
    }
    final next = plan?.todo.firstOrNull;
    if (plan == null || next == null) return const ChangeAnswered();
    final left = buildCapUsd - plan.spentUsd;
    if (left <= 0) return const ChangeAnswered();
    await _quiet(
      'mark working',
      () => _api.setWorking(aid, on: true, note: next.title, threadIds: []),
    );
    try {
      final AgentRunner runner;
      final Uri base;
      final _Snapshot snap;
      final List<OtherApp> others;
      final String system;
      final bool bundled;
      try {
        runner = _requireRunner();
        base = _baseUrl!();
        final listing = _api.listArtifacts();
        final reads = await Future.wait<Object?>([
          _firstSnapshot(aid, listing),
          _othersFrom(listing, exclude: aid),
          _prompts.toolSystem(),
          _isFromBundle(aid),
        ]);
        snap = reads[0]! as _Snapshot;
        others = reads[1]! as List<OtherApp>;
        system = reads[2]! as String;
        bundled = reads[3]! as bool;
      } on ClaxApiError catch (e) {
        if (e.unanswered) return const ChangeInterrupted();
        if (e.isNotFound && e.code == 'not_found') return const ChangeDropped();
        return ChangeFailed(e.message, answered: true);
      } on AgentError catch (e) {
        return ChangeFailed(e.message, answered: true);
      } on Object catch (e) {
        _log('continue on $aid failed before its run: $e');
        return ChangeFailed('$e', answered: true);
      }
      if (cancel.isCancelled) return const ChangeCancelled();
      final documents = PromptBuilder.trimDocuments(snap.docs);
      // The draft a change left when it planned the rest of itself.
      UnfinishedChange? carried;
      try {
        carried = await unfinished.get(buildDraftKey(aid));
      } on Object catch (e) {
        _log('the build draft of $aid is unavailable: $e');
      }
      final holds =
          carried != null &&
          carried.before.entries.every(
            (e) => fingerprint(snap.files[e.key]) == e.value,
          );
      final start = holds ? carried.edits : const <String, String?>{};
      final draftFiles = {...snap.files};
      for (final e in start.entries) {
        if (e.value case final v?) {
          draftFiles[e.key] = v;
        } else {
          draftFiles.remove(e.key);
        }
      }
      const caps = RunCaps.continueBuild;
      final scope = _RunCancel(cancelled);
      final ran = await _runTools(
        runner,
        base,
        scope,
        RunContext(
          kind: RequestKind.change,
          artifactId: aid,
          baseVersion: snap.artifact.currentVersion,
          title: snap.artifact.title,
          icon: snap.artifact.icon,
          capabilities: snap.artifact.capabilities,
          files: snap.files,
          shownDocs: {
            for (final c in documents)
              for (final d in c.docs) d.path: d.version,
          },
          cancelled: scope.future,
          clock: _clock,
          asksIcon: !draftFiles.containsKey(appIconPath),
        ),
        RunCaps(
          turns: caps.turns,
          renders: caps.renders,
          outputTokens: caps.outputTokens,
          costUsd: left < caps.costUsd ? left : caps.costUsd,
          wallClock: caps.wallClock,
        ),
        RunInput(
          system: system,
          first: _prompts.firstContinue(
            title: snap.artifact.title,
            plan: plan,
            files: draftFiles,
            others: others,
            documents: snap.docs,
            fromBundle: bundled,
            carried: carried == null
                ? null
                : holds
                ? CarriedDraft(
                    edited: carried.edits.keys.toList()..sort(),
                    diff: _diffOf(carried.edits, snap.files),
                    lastRender: carried.lastRender,
                  )
                : const CarriedDraft(
                    why: 'the app changed under its unfinished draft',
                  ),
          ),
        ),
        onProgress,
        agentContinueTiming,
        {'artifact': aid},
        kind: RunKind.continueBuild,
        fromBundle: bundled,
        plan: RunPlan(plans, aid, request: plan.request, seen: plan.revision),
        start: start,
        startTitle: holds ? carried.title : null,
        startCapabilities: holds ? carried.capabilities : null,
      );
      // A continue run has no one waiting on its question.
      await ran.ask.withdrawOpen();
      // The carried draft is done with once it landed, or when it no
      // longer fits the app.
      final landed = switch (ran.outcome) {
        RunFinished(version: _?) => true,
        _ => false,
      };
      if (carried != null && (!holds || landed)) {
        await _quiet(
          'forget the build draft',
          () => unfinished.remove([buildDraftKey(aid)]),
        );
      }
      return switch (ran.outcome) {
        RunFinished(:final version?) => ChangeLanded(version),
        RunFinished() => const ChangeAnswered(),
        RunGaveUp(:final reason) => ChangeFailed(
          ran.error?.message ?? reason,
          answered: true,
        ),
        RunCancelled() => const ChangeCancelled(),
        RunInterrupted() => const ChangeInterrupted(),
        RunDropped() => const ChangeDropped(),
      };
    } finally {
      await _quiet('clear working', () => _api.setWorking(aid, on: false));
    }
  }

  /// The plan of [aid], or null when it has none or it cannot be read.
  Future<Plan?> _plan(String aid) async {
    try {
      return await plans.get(aid);
    } on Object catch (e) {
      _log('the plan of $aid is unavailable: $e');
      return null;
    }
  }

  /// Takes every [AskedEntry] whose comments overlap [items] and withdraws
  /// its question, as [session]; a question that is closed or gone stays
  /// so.
  Future<void> _takeOverQuestions(
    List<FeedbackItem> items,
    String? session, {
    Set<String> keep = const {},
  }) async {
    final ids = {for (final i in items) i.commentId};
    final List<AskedEntry> entries;
    try {
      entries = await asked.all();
    } on Object catch (e) {
      _log('asked questions unavailable: $e');
      return;
    }
    for (final e in entries) {
      if (keep.contains(e.questionId) || !e.commentIds.any(ids.contains)) {
        continue;
      }
      try {
        await asked.take(e.questionId);
      } on Object catch (err) {
        _log('forgetting question ${e.questionId} failed: $err');
      }
      if (session == null) continue;
      try {
        await _api.withdrawQuestion(session, e.questionId);
      } on Object catch (err) {
        _log('withdrawing question ${e.questionId} failed: $err');
      }
    }
  }

  Future<ChangeOutcome> _changeTools(
    String aid,
    List<FeedbackItem> items,
    List<String> threadIds,
    String? session,
    Future<void>? cancelled,
    ProgressSink? onProgress,
    String? answers,
    UnfinishedChange? kept, {
    bool lost = false,
  }) async {
    final cancel = CancelWatch(cancelled);
    Future<ChangeOutcome> fail(String reason) async {
      final answered = await _replyFailure(
        aid,
        items,
        threadIds,
        session,
        reason,
      );
      return ChangeFailed(reason, answered: answered);
    }

    final AgentRunner runner;
    final Uri base;
    final _Snapshot snap;
    final List<OtherApp> others;
    final Uint8List? clip;
    final String system;
    final Plan? plan;
    final bool bundled;
    try {
      runner = _requireRunner();
      base = _baseUrl!();
      final listing = _api.listArtifacts();
      final reads = await Future.wait<Object?>([
        _firstSnapshot(aid, listing),
        _othersFrom(listing, exclude: aid),
        runner.sendsImages
            ? _clip(aid, items.first.threadId)
            : Future<Uint8List?>.value(),
        _prompts.toolSystem(),
        _plan(aid),
        _isFromBundle(aid),
      ]);
      bundled = reads[5]! as bool;
      snap = reads[0]! as _Snapshot;
      others = reads[1]! as List<OtherApp>;
      clip = reads[2] as Uint8List?;
      system = reads[3]! as String;
      plan = reads[4] as Plan?;
    } on ClaxApiError catch (e) {
      if (e.unanswered) {
        _log('change on $aid interrupted: $e');
        return const ChangeInterrupted();
      }
      if (e.isNotFound && e.code == 'not_found') {
        _log('change on $aid dropped: the app is gone');
        return const ChangeDropped();
      }
      return fail(e.message);
    } on AgentError catch (e) {
      return fail(e.message);
    } on Object catch (e) {
      _log('change on $aid failed before its run: $e');
      return fail('$e');
    }
    if (cancel.isCancelled) return const ChangeCancelled();

    final documents = PromptBuilder.trimDocuments(snap.docs);
    // Keep going: the stopped run's edits, where the app still holds what
    // they were made on.
    var start = const <String, String?>{};
    _Resume? resume;
    if (kept != null) {
      final holds = kept.before.entries.every(
        (e) => fingerprint(snap.files[e.key]) == e.value,
      );
      resume = holds ? _Resume.draft : _Resume.changed;
      if (resume == _Resume.draft) start = kept.edits;
    }
    // With nothing kept, the count and spend go on from the thread's last
    // limit reply.
    final carried = lost ? await _carriedFrom(aid, threadIds) : null;
    final spentBefore = kept?.spentUsd ?? carried?.spentUsd ?? 0;
    final stopsBefore = kept?.stops ?? carried?.stops ?? 0;
    final draftFiles = {...snap.files};
    for (final e in start.entries) {
      if (e.value case final v?) {
        draftFiles[e.key] = v;
      } else {
        draftFiles.remove(e.key);
      }
    }
    final left = changeCapUsd - spentBefore;
    const caps = RunCaps.change;
    final scope = _RunCancel(cancelled);
    final ran = await _runTools(
      runner,
      base,
      scope,
      RunContext(
        kind: RequestKind.change,
        artifactId: aid,
        baseVersion: snap.artifact.currentVersion,
        title: snap.artifact.title,
        icon: snap.artifact.icon,
        capabilities: snap.artifact.capabilities,
        files: snap.files,
        items: items,
        threadIds: threadIds,
        // This app's documents only: db_write pins by path alone.
        shownDocs: {
          for (final c in documents)
            for (final d in c.docs) d.path: d.version,
        },
        sessionId: session,
        cancelled: scope.future,
        clock: _clock,
        changeSpentUsd: spentBefore,
        changeStops: stopsBefore,
        changeNotes: kept?.notes ?? const [],
        startedOver: lost || resume == _Resume.changed,
      ),
      RunCaps(
        turns: caps.turns,
        renders: caps.renders,
        outputTokens: caps.outputTokens,
        costUsd: left < caps.costUsd ? left : caps.costUsd,
        wallClock: caps.wallClock,
      ),
      RunInput(
        system: system,
        first: _prompts.firstChange(
          title: snap.artifact.title,
          files: draftFiles,
          comments: [
            for (var i = 0; i < items.length; i++)
              CommentContext(
                threadId: items[i].threadId,
                author: items[i].author,
                body: items[i].body,
                anchorLine: anchorLine(items[i].anchor, items[i].version),
                clip: i == 0 ? clip : null,
              ),
          ],
          others: others,
          documents: snap.docs,
          answers: _joined([?answers, ?kept?.answers]),
          plan: plan,
          fromBundle: bundled,
          keepGoing: kept == null
              ? (lost
                    ? KeepGoingNote(
                        limit: '',
                        stops: stopsBefore,
                        notes: const [],
                        why: 'what that run left is no longer kept',
                      )
                    : null)
              : KeepGoingNote(
                  limit: kept.limit,
                  stops: kept.stops,
                  diff: resume == _Resume.draft
                      ? _diffOf(kept.edits, snap.files)
                      : null,
                  lastRender: resume == _Resume.draft ? kept.lastRender : null,
                  notes: kept.notes,
                  edited: resume == _Resume.draft
                      ? (kept.edits.keys.toList()..sort())
                      : null,
                  why: switch (resume!) {
                    _Resume.draft => null,
                    _Resume.changed =>
                      'the app changed under its unfinished draft',
                  },
                ),
        ),
      ),
      onProgress,
      agentChangeTiming,
      {'artifact': aid},
      fromBundle: bundled,
      kind: RunKind.change,
      plan: RunPlan(
        plans,
        aid,
        request: snap.artifact.title,
        seen: plan?.revision,
        freshBuild: true,
      ),
      start: start,
      startTitle: resume == _Resume.draft ? kept?.title : null,
      startCapabilities: resume == _Resume.draft ? kept?.capabilities : null,
    );
    await _keepUnfinished(
      aid,
      items,
      threadIds,
      snap.files,
      spentBefore,
      ran,
      earlierNotes: kept?.notes ?? const [],
      earlierAnswers: kept?.answers,
      earlierStops: stopsBefore,
    );

    final outcome = ran.outcome;
    if (outcome is RunGaveUp || outcome is RunDropped) {
      // Nothing waits on the question any more: the change failed or its
      // app is gone.
      await ran.ask.withdrawOpen();
    }
    switch (outcome) {
      case RunFinished(:final version, :final answered):
        return version == null
            ? ChangeAnswered(answered: answered)
            : ChangeLanded(version, answered: answered);
      case RunGaveUp(planned: true, :final answered):
        // The build builds what is left; the reply said so.
        return ChangeAnswered(answered: answered);
      case RunGaveUp(:final reason, :final answered):
        if (ran.byTool) return ChangeFailed(reason, answered: answered);
        // The runner ended it (a refusal, a broken turn, a model failure):
        // nothing has replied yet.
        return fail(ran.error?.message ?? reason);
      case RunCancelled():
        return const ChangeCancelled();
      case RunInterrupted():
        return const ChangeInterrupted();
      case RunDropped():
        return const ChangeDropped();
    }
  }

  /// Replies [failureReply] of [reason] on each of [threadIds] but those
  /// with a newer viewer comment, then acknowledges [items] to [session].
  /// Returns whether every reply was taken.
  Future<bool> _replyFailure(
    String aid,
    List<FeedbackItem> items,
    List<String> threadIds,
    String? session,
    String reason,
  ) async {
    final newer = await newerComments(_api, aid, threadIds, {
      for (final i in items) i.commentId,
    }, log: _log);
    var answered = true;
    for (final t in threadIds) {
      if (newer.contains(t)) {
        _log('$aid/$t has a newer comment; its own run answers it');
        continue;
      }
      try {
        await _api.replyAsAgent(aid, t, failureReply(reason));
      } on Object catch (e) {
        _log('reply on $aid/$t failed: $e');
        answered = false;
      }
    }
    if (session != null) {
      await _quiet(
        'acknowledge',
        () => _api.ackFeedback(session, [for (final i in items) i.commentId]),
      );
    }
    return answered;
  }

  /// The runner of a tool-loop run; throws [AgentError] when this loop
  /// cannot run one (no runner, or no inspector to render with).
  AgentRunner _requireRunner() {
    final make = _runner;
    if (make == null || _inspector == null || _baseUrl == null) {
      throw const AgentError(
        'The agent cannot check its work here: no runner or renderer is set '
        'up.',
      );
    }
    return make();
  }

  /// Runs [runner] over a fresh [Toolbox] for [ctx], timed as [timing].
  /// Ends the run however it ended: the draft artifact is deleted, and a
  /// model failure becomes the outcome ([RunGaveUp], or [RunCancelled]
  /// when the run was cancelled).
  Future<_Ran> _runTools(
    AgentRunner runner,
    Uri base,
    _RunCancel scope,
    RunContext ctx,
    RunCaps caps,
    RunInput input,
    ProgressSink? onProgress,
    String timing,
    Map<String, Object?> extra, {
    required RunKind kind,
    RunPlan? plan,
    bool fromBundle = false,
    Map<String, String?> start = const {},
    String? startTitle,
    Map<String, Object?>? startCapabilities,
  }) async {
    // [_requireRunner] made [runner], so the inspector is there.
    final inspector = _inspector!;
    final model = _model();
    final narrations = <String>[];
    final guarded = _guarded(onProgress);
    void sink(AgentProgress e) {
      if (e case Narration(:final text)) narrations.add(text);
      guarded?.call(e);
    }

    final render = RenderTool(_api, inspector, baseUrl: base);
    final finish = FinishTool(_api, ledger, render: render, log: _log);
    final giveUp = GiveUpTool(
      _api,
      render: render,
      finish: finish,
      log: _log,
    );
    final ask = AskTool(
      _api,
      asked,
      maxWaitNew: askWaitNewApp,
      maxWaitChange: askWaitChange,
      onAsked: _onAsked,
    );
    final draft = Draft(
      files: ctx.files,
      title: ctx.title,
      capabilities: ctx.capabilities,
      baseVersion: ctx.baseVersion,
    );
    for (final e in start.entries) {
      if (e.value case final v?) {
        draft.writeFile(e.key, v);
      } else {
        draft.deleteFile(e.key);
      }
    }
    if (startTitle != null || startCapabilities != null) {
      draft.setMeta(title: startTitle, capabilities: startCapabilities);
    }
    final state = RunState(
      ctx: ctx,
      draft: draft,
      budget: RunBudget(caps, model, _clock),
      progress: sink,
      readConsent: _readConsent,
      narrations: narrations,
    );
    var byTool = false;
    void ended() => byTool = byTool || state.ended;
    final toolbox = Toolbox(state, [
      if (plan != null) ...[PlanSetTool(plan), PlanMarkTool(plan)],
      AddKitTool(),
      ...fileTools(),
      ...dataTools(_api, fromBundle: fromBundle),
      render,
      ask,
      _Ending(finish, ended),
      _Ending(giveUp, ended),
    ]);
    finish.attach(toolbox);
    toolbox.capHandler = (cap, narration) async {
      final o = await finish.capStop(cap, narration: narration);
      byTool = true;
      return o;
    };

    sink(const AttemptStarted(1));
    _timings.start(timing);
    RunOutcome outcome;
    AgentError? error;
    try {
      outcome = await runner.run(input, toolbox);
    } on AgentError catch (e) {
      error = e;
      outcome =
          state.outcome ??
          (e.kind == AgentErrorKind.cancelled
              ? const RunCancelled()
              : RunGaveUp(e.message, answered: false));
    } on Object catch (e) {
      _log('the $timing run failed: $e');
      error = AgentError('$e');
      outcome = state.outcome ?? RunGaveUp('$e', answered: false);
    } finally {
      // A cancel after this point (the worker stopping later) no longer
      // reaches the run's tools: its question stays parked.
      scope.end();
      final b = state.budget;
      _timings.stop(
        timing,
        extra: {
          ...extra,
          'model': model,
          'backend': runner.name,
          'turns': b.turns,
          'renders': b.renders,
          'input_tokens': b.usage.input,
          'cache_read_tokens': b.usage.cacheRead,
          'cache_write_tokens': b.usage.cacheWrite,
          'output_tokens': b.usage.output,
          'cost_usd': b.costUsd,
          'run': kind.name,
          'outcome': switch (state.outcome) {
            RunFinished() => 'finished',
            RunGaveUp() => 'gave up',
            RunCancelled() => 'cancelled',
            RunInterrupted() => 'interrupted',
            RunDropped() => 'dropped',
            null => error == null ? 'ended' : 'failed',
          },
        },
      );
    }
    state.outcome ??= outcome;
    // Every end deletes the draft, the runner's own ends included.
    await render.cleanup();
    if (plan != null && kind != RunKind.change) {
      // A build's runs count against its cost cap.
      try {
        await plans.spend(plan.key, state.budget.costUsd);
      } on Object catch (e) {
        _log('counting the cost of ${plan.key} failed: $e');
      }
    }
    return (
      outcome: outcome,
      byTool: byTool,
      error: error,
      ask: ask,
      state: state,
      narrations: narrations,
      finish: finish,
      giveUp: giveUp,
    );
  }

  ProgressSink? _guarded(ProgressSink? onProgress) => onProgress == null
      ? null
      : (event) {
          try {
            onProgress(event);
          } on Object catch (e) {
            _log('the progress listener failed: $e');
          }
        };

  Future<ChangeOutcome> _changeSingle(
    AgentBackend backend,
    String aid,
    List<FeedbackItem> items,
    List<String> threadIds,
    String? session,
    Future<void>? cancelled,
    ProgressSink? onProgress,
  ) async {
    final cancel = CancelWatch(cancelled);
    final commentIds = {for (final i in items) i.commentId};

    var newerAtPublish = const <String>{};
    Future<ChangeOutcome> finish(
      String reply,
      ChangeOutcome Function(bool answered) of, {
      bool resolve = false,
    }) async {
      final newer = {
        ...newerAtPublish,
        ...await newerComments(_api, aid, threadIds, commentIds, log: _log),
      };
      var answered = true;
      for (final t in threadIds) {
        if (newer.contains(t)) {
          _log('$aid/$t has a newer comment; its own run answers it');
          continue;
        }
        try {
          await _api.replyAsAgent(aid, t, reply);
        } on Object catch (e) {
          _log('reply on $aid/$t failed: $e');
          answered = false;
          continue;
        }
        if (resolve) {
          await _quiet('resolve $t', () => _api.resolveAsAgent(aid, t));
        }
      }
      if (session != null) {
        await _quiet(
          'acknowledge',
          () => _api.ackFeedback(session, [for (final i in items) i.commentId]),
        );
      }
      return of(answered);
    }

    Future<ChangeOutcome> fail(String reason) => finish(
      failureReply(reason),
      (answered) => ChangeFailed(reason, answered: answered),
    );

    try {
      final images = _sendsImage(backend);
      // Every read before the model call runs at once.
      final listing = _api.listArtifacts();
      final reads = await Future.wait<Object?>([
        _firstSnapshot(aid, listing),
        _othersFrom(listing, exclude: aid),
        images ? _clip(aid, items.first.threadId) : Future<Uint8List?>.value(),
        _prompts.system(),
        _isFromBundle(aid),
      ]);
      final bundled = reads[4]! as bool;
      var snap = reads[0]! as _Snapshot;
      final others = reads[1]! as List<OtherApp>;
      final clip = reads[2] as Uint8List?;
      final system = reads[3]! as String;
      String? retryReason;
      var reran = false;
      var attempt = 0;
      while (true) {
        cancel.check();
        final text = _prompts.change(
          title: snap.artifact.title,
          files: snap.files,
          comments: [
            for (var i = 0; i < items.length; i++)
              CommentContext(
                threadId: items[i].threadId,
                author: items[i].author,
                body: items[i].body,
                anchorLine: anchorLine(items[i].anchor, items[i].version),
                clip: i == 0 ? clip : null,
              ),
          ],
          others: others,
          documents: snap.docs,
          retryReason: retryReason,
          fromBundle: bundled,
        );
        final EditResult r;
        try {
          r = await _run(
            ++attempt,
            backend,
            RequestKind.change,
            system,
            text,
            clip,
            cancelled,
            onProgress,
            {'artifact': aid},
            fromBundle: bundled,
          );
        } on AgentError catch (e) {
          if (e.kind == AgentErrorKind.cancelled) {
            return const ChangeCancelled();
          }
          return await fail(e.message);
        }
        if (cancel.isCancelled) return const ChangeCancelled();

        // Only what the prompt carried counts as shown.
        final shown = {
          for (final c in PromptBuilder.trimDocuments(snap.docs))
            for (final d in c.docs) d.path: d.version,
        };
        final Map<String, String?> changed;
        try {
          validateResult(
            r,
            kind: RequestKind.change,
            existingPaths: snap.files.keys.toSet(),
          );
          changed = applyResult(snap.files, r);
          await _refuseUnshown(aid, r.db, shown);
        } on ResultInvalid catch (e) {
          if (retryReason != null) return await fail(e.reason);
          retryReason = e.reason;
          continue;
        } on EditMiss catch (e) {
          if (retryReason != null) return await fail(e.reason);
          retryReason = e.reason;
          continue;
        }
        if (cancel.isCancelled) return const ChangeCancelled();

        final v = snap.artifact.currentVersion;
        int? landed;
        ChangeRecord? record;
        if (_changesVersion(snap.artifact, snap.files, changed, r)) {
          // A thread the person wrote on again while this ran is left out
          // of addresses: this version does not answer its newer comment.
          newerAtPublish = await newerComments(
            _api,
            aid,
            threadIds,
            commentIds,
            log: _log,
          );
          final addresses = [
            for (final t in threadIds)
              if (!newerAtPublish.contains(t)) t,
          ];
          record = ChangeRecord(
            artifactId: aid,
            base: v,
            addresses: addresses,
            answered: {
              for (final t in threadIds)
                t: [
                  for (final i in items)
                    if (i.threadId == t) i.commentId,
                ],
            },
            reply: r.reply,
            resolve: r.resolve,
            db: r.db,
            pins: {
              for (final o in r.db)
                '${o.collection}/${o.docId}':
                    shown['${o.collection}/${o.docId}'],
            },
            declared: Declaration.of(snap.artifact),
          );
          await _record(record);
          try {
            final res = await _api.publish(
              aid,
              PublishRequest(
                ifVersion: v,
                title: r.title,
                capabilities: r.capabilities,
                note: r.reply,
                files: changed,
                addresses: addresses,
              ),
            );
            landed = res.version.n;
          } on ClaxApiError catch (e) {
            // Without an answer the publish may have landed: the record
            // stays for recovery to settle.
            if (!e.unanswered) await _forget(aid, v);
            if (e.isConflict && !reran) {
              reran = true;
              snap = await _snapshot(aid);
              continue;
            }
            rethrow;
          }
        }

        if (r.db.isNotEmpty) {
          String? refused;
          try {
            refused = await writeStagedDocs(_api, aid, r.db, shown);
          } on ClaxApiError catch (e) {
            // No answer, or the app is gone: as for any call.
            if (e.unanswered ||
                (e.code == 'not_found' && e.body['path'] == null)) {
              rethrow;
            }
            // Any other failure (a re-read after a conflict) is a refusal:
            // the landed version is undone, as finish does.
            refused = e.message;
          }
          if (refused != null) {
            if (landed != null) {
              try {
                await _api.rollBack(
                  aid,
                  v,
                  ifVersion: landed,
                  declared: record?.declared,
                );
              } on ClaxApiError catch (e) {
                if (e.unanswered) return const ChangeInterrupted();
                _log('restoring $aid to version $v failed: $e');
              }
              await _forget(aid, v);
            }
            return await fail("its data couldn't be updated: $refused");
          }
          if (record != null) {
            record.docsDone = true;
            await _record(record);
          }
        }
        return await finish(
          r.reply,
          (answered) => landed == null
              ? ChangeAnswered(answered: answered)
              : ChangeLanded(landed, answered: answered),
          resolve: r.resolve,
        );
      }
    } on AgentError catch (e) {
      if (e.kind == AgentErrorKind.cancelled) return const ChangeCancelled();
      return await fail(e.message);
    } on ClaxApiError catch (e) {
      if (e.unanswered) {
        _log('change on $aid interrupted: $e');
        return const ChangeInterrupted();
      }
      if (e.isNotFound && e.code == 'not_found') {
        _log('change on $aid dropped: the artifact is gone');
        return const ChangeDropped();
      }
      return await fail(e.message);
    } on Object catch (e) {
      _log('change on $aid failed: $e');
      return await fail('$e');
    }
  }

  /// For recovery: settles what [ledger] says this app published on [aid],
  /// given the artifact's [stranded] threads, and returns the comment IDs
  /// a standing version already answered, the threads it replied on, and
  /// the threads of changes it undid (whose comments run again, and whose
  /// undone version must not be taken as their answer).
  ///
  /// A record whose version is not there (the publish never landed) is
  /// dropped. For one whose version is there: document writes that may not
  /// have run are sent again with their pins (a conflict or a refused
  /// unpinned write means they ran); a refusal restores the version before
  /// it, when it is still current, and drops the record, so its comments
  /// run again. Then each addressed thread whose pending comments the
  /// record all answered gets the record's reply (and resolve). Throws
  /// [ClaxApiError] when the daemon gives no answer.
  Future<({Set<String> answered, Set<String> replied, Set<String> undone})>
  settle(String aid, List<ThreadView> stranded) async {
    final answered = <String>{};
    final replied = <String>{};
    final undone = <String>{};
    final List<ChangeRecord> records;
    try {
      records = await ledger.of(aid);
    } on Object catch (e) {
      _log('change ledger unavailable: $e');
      return (answered: answered, replied: replied, undone: undone);
    }
    if (records.isEmpty) {
      return (answered: answered, replied: replied, undone: undone);
    }
    final detail = await _api.getArtifact(aid);
    for (final r in records) {
      final v = detail.versions
          .where((x) => x.n == r.base + 1 && madeBy(x, r))
          .firstOrNull;
      if (v == null) {
        await _forget(aid, r.base);
        continue;
      }
      switch (await _settleDocs(aid, r, v, detail)) {
        case _Docs.undone:
          undone.addAll(r.answered.keys);
          continue;
        case _Docs.ambiguous:
          undone.addAll(r.answered.keys);
          for (final t in stranded) {
            final done = r.answered[t.id] ?? const [];
            if (!r.answered.containsKey(t.id) ||
                !pendingComments(t).every((c) => done.contains(c.id))) {
              continue;
            }
            await _api.replyAsAgent(aid, t.id, _ambiguousReply);
            replied.add(t.id);
          }
          continue;
        case _Docs.done:
          break;
      }
      for (final ids in r.answered.values) {
        answered.addAll(ids);
      }
      for (final t in stranded) {
        if (!r.addresses.contains(t.id) || replied.contains(t.id)) continue;
        final done = r.answered[t.id] ?? const [];
        if (!pendingComments(t).every((c) => done.contains(c.id))) continue;
        await _api.replyAsAgent(aid, t.id, r.reply);
        replied.add(t.id);
        if (r.resolve) {
          await _quiet('resolve ${t.id}', () => _api.resolveAsAgent(aid, t.id));
        }
      }
    }
    return (answered: answered, replied: replied, undone: undone);
  }

  static final _ambiguousReply = failureReply(
    'its data may have changed while it was being saved; nothing was '
    'changed. Send it again to retry.',
  );

  /// Finishes the document writes of record [r], whose version [v]
  /// stands: sends them again with their pins unless they are done. A
  /// refusal undoes the version ([_Docs.undone]); a pin that moved over
  /// documents that do not hold the writes undoes it too
  /// ([_Docs.ambiguous]: whether the batch ran is unknown). Throws the
  /// [ClaxApiError] of a call that got no answer.
  Future<_Docs> _settleDocs(
    String aid,
    ChangeRecord r,
    VersionInfo v,
    ArtifactDetail detail,
  ) async {
    if (r.docsDone || r.db.isEmpty) return _Docs.done;
    try {
      await _api.batchDocs(aid, [
        for (final o in r.db)
          DocWrite(
            op: o.op,
            collection: o.collection,
            docId: o.docId,
            data: o.data,
            ifVersion: r.pins['${o.collection}/${o.docId}'],
          ),
      ]);
    } on ClaxApiError catch (e) {
      if (e.unanswered) rethrow;
      final pinMoved = e.isConflict || e.code == 'if_version_required';
      if (!pinMoved) {
        // Refused: undo the version; its comments run again.
        _log('documents of $aid version ${v.n} refused: $e');
        await _restore(aid, r, v.n, detail.artifact.currentVersion);
        await _forget(aid, r.base);
        return _Docs.undone;
      }
      if (!await _written(aid, r.db)) {
        // A pin moved and the documents do not hold the writes: the page
        // wrote them, and whether the batch ran is unknown. Undo the
        // version and fail, rather than skip a migration.
        _log('documents of $aid version ${v.n} ambiguous: $e');
        await _restore(aid, r, v.n, detail.artifact.currentVersion);
        await _forget(aid, r.base);
        return _Docs.ambiguous;
      }
      // The documents hold the writes: the batch already ran.
    }
    r.docsDone = true;
    await _record(r);
    return _Docs.done;
  }

  /// For a late answer about to run [e]'s comments again: settles a change
  /// that already ran them with this answer and landed but never replied
  /// (its publish lost its answer, the run ended interrupted). Such a
  /// record of [ledger] answered every comment of [e], its version stands
  /// ([madeBy]), and no thread it addresses has an agent comment made at
  /// or after that version (an unreadable time counts as replied, so
  /// nothing runs on a guess). Its
  /// documents are finished as [settle] does, then its reply (and resolve)
  /// is posted on the threads whose latest viewer comment is still one of
  /// [e]'s. Returns true when the change was settled this way (or failed
  /// with the reply that says so), and the answer must not run again;
  /// false when there is no such record, or its version was undone. Throws
  /// [ClaxApiError] when the daemon gives no answer.
  Future<bool> settleAnswered(String aid, AskedEntry e) async {
    final List<ChangeRecord> records;
    try {
      records = await ledger.of(aid);
    } on Object catch (err) {
      _log('change ledger unavailable: $err');
      return false;
    }
    final ids = e.commentIds.toSet();
    final candidates = [
      for (final r in records)
        if (ids.every((c) => r.answered.values.any((l) => l.contains(c)))) r,
    ];
    if (candidates.isEmpty) return false;
    final detail = await _api.getArtifact(aid);
    final threads = await _api.threads(aid);
    for (final r in candidates) {
      final v = detail.versions
          .where((x) => x.n == r.base + 1 && madeBy(x, r))
          .firstOrNull;
      if (v == null) continue;
      final landedAt = DateTime.tryParse(v.createdAt);
      final mine = [
        for (final t in threads)
          if (r.addresses.contains(t.id) && e.threadIds.contains(t.id)) t,
      ];
      // Replied when an agent comment came at or after the version: the
      // parked run's reply came before it.
      final replied = mine.any(
        (t) => t.comments.any((c) {
          if (!c.byAgent) return false;
          final at = DateTime.tryParse(c.createdAt);
          return landedAt == null || at == null || !at.isBefore(landedAt);
        }),
      );
      if (mine.isEmpty || replied) continue;
      final docs = await _settleDocs(aid, r, v, detail);
      if (docs == _Docs.undone) return false;
      for (final t in mine) {
        final viewer = [
          for (final c in t.comments)
            if (c.authorKind == 'viewer') c,
        ];
        if (viewer.isEmpty || !ids.contains(viewer.last.id)) continue;
        if (docs == _Docs.ambiguous) {
          await _api.replyAsAgent(aid, t.id, _ambiguousReply);
          continue;
        }
        await _api.replyAsAgent(aid, t.id, r.reply);
        if (r.resolve) {
          await _quiet('resolve ${t.id}', () => _api.resolveAsAgent(aid, t.id));
        }
      }
      _log('late answer on $aid: version ${v.n} had landed; settled');
      return true;
    }
    return false;
  }

  /// Whether the documents hold [ops]' writes, as finish judges it
  /// ([holdsWrite]: an update merged as the daemon merges it).
  Future<bool> _written(String aid, List<DbOp> ops) async {
    final docs = await Future.wait([
      for (final o in ops) _api.getDoc(aid, '${o.collection}/${o.docId}'),
    ]);
    for (var i = 0; i < ops.length; i++) {
      if (!holdsWrite(docs[i], ops[i])) return false;
    }
    return true;
  }

  /// Undoes record [r]'s version [landed] while it is still [current]:
  /// back to [ChangeRecord.base] with what it declared.
  Future<void> _restore(
    String aid,
    ChangeRecord r,
    int landed,
    int current,
  ) async {
    if (current != landed) return;
    try {
      await _api.rollBack(
        aid,
        r.base,
        ifVersion: landed,
        declared: r.declared,
      );
    } on ClaxApiError catch (e) {
      if (e.unanswered) rethrow;
      _log('restoring $aid to version ${r.base} failed: $e');
    }
  }

  Future<void> _record(ChangeRecord r) async {
    try {
      await ledger.put(r);
    } on Object catch (e) {
      _log('change ledger write failed: $e');
    }
  }

  Future<void> _forget(String aid, int base) async {
    try {
      await ledger.remove(aid, base);
    } on Object catch (e) {
      _log('change ledger write failed: $e');
    }
  }

  /// The first read of a change: the artifact from [listing] (shared with
  /// the other apps' summaries), then its current version's text files,
  /// with the app's own documents read alongside.
  Future<_Snapshot> _firstSnapshot(
    String aid,
    Future<List<ArtifactSummary>> listing,
  ) async {
    final docs = _ownDocuments(aid);
    final all = await listing;
    final artifact =
        all.where((a) => a.id == aid).firstOrNull ??
        (throw const ClaxApiError(
          status: 404,
          code: 'not_found',
          message: 'the app is gone',
        ));
    final files = await _api.readFiles(aid, artifact.currentVersion);
    return (artifact: artifact, files: files, docs: await docs);
  }

  /// A fresh read of the artifact, its files and documents, after a
  /// publish conflict.
  Future<_Snapshot> _snapshot(String aid) async {
    final docs = _ownDocuments(aid);
    final detail = await _api.getArtifact(aid);
    final files = await _api.readFiles(aid, detail.artifact.currentVersion);
    return (artifact: detail.artifact, files: files, docs: await docs);
  }

  /// Throws [ResultInvalid] when [ops] write a document that exists but
  /// is not among [shown]: its contents were never in the prompt.
  Future<void> _refuseUnshown(
    String aid,
    List<DbOp> ops,
    Map<String, int> shown,
  ) async {
    final unseen = {
      for (final o in ops)
        if (!shown.containsKey('${o.collection}/${o.docId}'))
          '${o.collection}/${o.docId}',
    }.toList();
    if (unseen.isEmpty) return;
    final found = await Future.wait([
      for (final p in unseen) _api.getDoc(aid, p),
    ]);
    for (var i = 0; i < unseen.length; i++) {
      if (found[i] != null) {
        throw ResultInvalid(
          'db operation on ${jsonEncode(unseen[i])}: the document exists but '
          'was not shown; write only documents shown in the prompt, or new '
          'ones',
        );
      }
    }
  }

  /// Whether applying [changed] (and [r]'s title and capabilities) makes a
  /// version different from the current one.
  static bool _changesVersion(
    ArtifactSummary current,
    Map<String, String> files,
    Map<String, String?> changed,
    EditResult r,
  ) {
    for (final e in changed.entries) {
      if (e.value == null || files[e.key] != e.value) return true;
    }
    if (r.title != null && r.title != current.title) return true;
    if (r.capabilities != null &&
        jsonEncode(r.capabilities) != jsonEncode(current.capabilities)) {
      return true;
    }
    return false;
  }

  /// Runs [backend] once, as attempt [attempt] of this request, telling
  /// [onProgress] first.
  Future<EditResult> _run(
    int attempt,
    AgentBackend backend,
    RequestKind kind,
    String system,
    String text,
    Uint8List? image,
    Future<void>? cancelled,
    ProgressSink? onProgress,
    Map<String, Object?> extra, {
    bool fromBundle = false,
  }) async {
    final name = kind == RequestKind.newApp
        ? agentNewTiming
        : agentChangeTiming;
    final model = _model();
    final ProgressSink? sink = onProgress == null
        ? null
        : (event) {
            try {
              onProgress(event);
            } on Object catch (e) {
              _log('the progress listener failed: $e');
            }
          };
    sink?.call(AttemptStarted(attempt));
    _timings.start(name);
    try {
      return await backend.run(
        AgentRequest(
          kind: kind,
          model: model,
          systemPrompt: system,
          userText: text,
          image: image,
          cancelled: cancelled,
          onProgress: sink,
          fromBundle: fromBundle,
        ),
      );
    } finally {
      _timings.stop(
        name,
        extra: {...extra, 'model': model, 'backend': backend.name},
      );
    }
  }

  static String _retryText(String reason) =>
      'Your previous result could not be applied: $reason\n'
      'Answer again. This time send every file as a whole file in `files`, '
      'and leave `edits` empty.\n';

  Future<Uint8List?> _clip(String aid, String tid) async {
    try {
      return await _api.clip(aid, tid);
    } on ClaxApiError catch (e) {
      _log('clip of $aid/$tid unavailable: $e');
      return null;
    }
  }

  /// The changed app's collections with up to [PromptBuilder.maxOwnDocs]
  /// documents each, read at once; none when they cannot be read.
  Future<List<AppCollection>> _ownDocuments(String aid) async {
    try {
      final colls = await _api.collections(aid);
      return await Future.wait([
        for (final c in colls.take(PromptBuilder.maxOwnCollections))
          _api
              .listDocs(aid, c.path, limit: PromptBuilder.maxOwnDocs)
              .then(
                (docs) => (
                  path: c.path,
                  count: c.count,
                  docs: <AppDocument>[
                    for (final d in docs)
                      (path: d.path, version: d.version, data: d.data),
                  ],
                ),
              ),
      ]);
    } on Object catch (e) {
      _log('documents of $aid unavailable: $e');
      return const [];
    }
  }

  /// The person's other apps that declare `db`, by ID and title only, in
  /// [listing]'s order, without [exclude]; none when they cannot be
  /// listed. Nothing of their documents is read here: the agent reads them
  /// through the data tools, after the owner's own consent.
  Future<List<OtherApp>> _othersFrom(
    Future<List<ArtifactSummary>> listing, {
    String? exclude,
  }) async {
    final List<ArtifactSummary> all;
    try {
      all = await listing;
    } on Object catch (e) {
      _log('other apps unavailable: $e');
      return const [];
    }
    return [
      for (final a in all)
        if (a.id != exclude && a.declaresDb && a.kind == 'html')
          OtherApp(id: a.id, title: a.title, collections: const []),
    ].take(PromptBuilder.maxOthers).toList();
  }

  static String _workingNote(List<FeedbackItem> items) {
    final first = items.first.body.replaceAll(RegExp(r'\s+'), ' ').trim();
    return first.length > 80 ? '${first.substring(0, 80)}…' : first;
  }

  Future<void> _quiet(String what, Future<void> Function() call) async {
    try {
      await call();
    } on Object catch (e) {
      _log('$what failed: $e');
    }
  }
}

/// How [AgentLoop._settleDocs] left a record's documents.
enum _Docs { done, undone, ambiguous }

/// A run's cancel signal: [future] completes when the caller's signal
/// fires while the run is going, never after [end]. The worker passes one
/// signal to every run, and a run's tools (`ask` withdrawing its question)
/// must not act on a cancel that comes after the run ended.
class _RunCancel {
  _RunCancel(Future<void>? outer) {
    outer?.then((_) => _fire(), onError: (Object _) => _fire());
  }

  final _fired = Completer<void>();
  var _over = false;

  Future<void> get future => _fired.future;

  void _fire() {
    if (!_over && !_fired.isCompleted) _fired.complete();
  }

  void end() => _over = true;
}

/// A tool that ends a run (`finish`, `give_up`), telling [ended] after
/// each call, so the loop knows the run's end came from a tool, which
/// replied itself.
class _Ending implements Tool {
  _Ending(this.inner, this.ended);

  final Tool inner;
  final void Function() ended;

  @override
  ToolSpec get spec => inner.spec;

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    try {
      return await inner.call(input, s);
    } finally {
      ended();
    }
  }
}

/// The viewer comments of [t] after its last agent comment: the ones no
/// reply has answered yet.
List<ThreadComment> pendingComments(ThreadView t) {
  final since = t.comments.lastIndexWhere((c) => c.byAgent) + 1;
  return [
    for (final c in t.comments.skip(since))
      if (c.authorKind == 'viewer') c,
  ];
}

/// The comment's anchor as the daemon's feedback payload renders it on its
/// "Anchored on:" line (`Anchor::summary` in crates/clax-core/src/anchor.rs),
/// then two spaces and `(v<version>)`.
String anchorLine(Map<String, Object?> anchor, int version) =>
    '${anchorSummary(anchor)}  (v$version)';

/// `the whole artifact` for an `artifact` anchor; otherwise the route and
/// ` › ` when there is one, the file and ` › ` when it is not `index.html`,
/// the selector (`custom:<name>`, or `area in <selector> (W × H)`), then
/// the quote (its `label` when it has one) in `«»`, whitespace collapsed and
/// cut to 120 characters.
String anchorSummary(Map<String, Object?> anchor) {
  final kind = anchor['kind'] as String? ?? 'artifact';
  if (kind == 'artifact') return 'the whole artifact';
  final selector = anchor['selector'] as String? ?? '';
  var target = switch (kind) {
    'custom' => 'custom:${anchor['custom_name'] as String? ?? ''}',
    'area' => switch (anchor['area']) {
      final Map<Object?, Object?> a =>
        'area in $selector (${_pct(a['w'])} × ${_pct(a['h'])})',
      _ => 'area in $selector',
    },
    _ => selector,
  };
  final file = anchor['file'] as String? ?? 'index.html';
  if (file != 'index.html') target = '$file › $target';
  if (anchor['route'] case final String route) target = '$route › $target';
  final quote = _collapse(
    anchor['label'] as String? ?? anchor['quote'] as String? ?? '',
  );
  if (quote.isEmpty) return target;
  return '$target  «${_cap(quote.replaceAll(RegExp('[«»]'), '"'), 120)}»';
}

String _collapse(String s) =>
    s.split(RegExp(r'\s+')).where((w) => w.isNotEmpty).join(' ');

String _cap(String s, int n) {
  final runes = s.runes.toList();
  return runes.length <= n ? s : '${String.fromCharCodes(runes.take(n))}…';
}

String _pct(Object? f) {
  final v = (f as num?)?.toDouble() ?? 0;
  final p = (v.clamp(0.0, 1.0) * 100).round();
  return p == 0 && v > 0 ? '<1%' : '$p%';
}
