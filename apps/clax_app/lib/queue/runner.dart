/// The queue's effect runner (spec 2026-10-19-in-process-core-design
/// §7.1, §7.3–§7.5): the only code in the worker that awaits, and the only
/// code that knows the core's routes (docs/contract.md, "Jobs"). It runs
/// each effect `update` emits and feeds its result back as the matching
/// message, nothing more: it decides nothing about a job.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart' show kDebugMode;

import '../agent/backend.dart' show AgentError, AgentErrorKind;
import '../clax/api.dart' show ClaxApiError;
import '../clax/sse.dart' show SseMessage;
import '../clax/transport.dart';
import '../core/core_transport.dart';
import '../core/json_import.dart';
import 'codec.dart'
    show RenderAnswered, RenderGone, RenderRequest, ToUi, UiEffect;
import 'effect.dart';
import 'model.dart';
import 'msg.dart';
import 'render.dart';
import 'worker_main.dart' show WorkerContext, WorkerRunner;

/// The agent's turns and tools (lib/agent), one at a time.
abstract interface class AgentPort {
  /// Runs one model turn of job [jobId]. Stops when [cancelled] completes.
  /// Throws [AgentError] when the model fails; kind `network` when it could
  /// not be reached.
  Future<TurnOutcome> turn(
    JobId jobId,
    TurnInput input,
    Future<void> cancelled,
  );

  /// Runs [call], one of turn [turn]'s calls but `finish`, `render` and
  /// `ask`. Stops when [cancelled] completes. An [Exception] it throws is
  /// the call's error result.
  Future<ToolResult> tool(
    JobId jobId,
    int turn,
    OtherCall call,
    Future<void> cancelled,
  );

  /// What the `render` call [callId] of turn [turn] draws, [planned] as
  /// the turn named it: the job's draft as the calls before it left it
  /// (they run in order, before it), served for the UI isolate to load,
  /// with its digest and test files. An [Exception] it throws answers the
  /// render as failed with its text.
  Future<RenderSpec> renderSpec(
    JobId jobId,
    int turn,
    String callId,
    RenderSpec planned,
  );

  /// What the `finish` call [callId] of turn [turn] publishes, [planned]
  /// as the turn named it: the job's draft as the calls before it left it.
  /// The plan's `base` is [planned]'s. An [Exception] it throws refuses the
  /// publish without asking the core: a [PublishRefused] as its code, any
  /// other as [Invalid] `no_draft`. The finish call is then answered with
  /// the refusal, which the next turn plans from.
  Future<PublishPlan> publishPlan(
    JobId jobId,
    int turn,
    String callId,
    PublishPlan planned,
  );

  /// Hears what the `render` call [callId] of turn [turn] drew, once the
  /// UI isolate answered it and before the queue hears it: the draft the
  /// [renderSpec] served counts as rendered so.
  void rendered(JobId jobId, int turn, String callId, RenderOutcome outcome);

  /// Lets go of what job [jobId]'s agent holds in this worker (its draft,
  /// history and budget): the job ended, or its work stopped. A later turn
  /// or call of it rebuilds them from the job's row.
  void forget(JobId jobId);
}

/// A publish [AgentPort.publishPlan] refuses before the core is asked:
/// the finish call is answered with [code] and [message].
final class PublishRefused implements Exception {
  const PublishRefused(this.code, this.message);
  final String code;
  final String message;
  @override
  String toString() => message;
}

/// Where a debug build's kill hook ([KillHook]) holds a `Publish`.
enum KillPoint {
  /// Before the step's commit is sent: the core holds no version of it.
  beforePublishCommit('before-publish-commit'),

  /// Once the core answered the commit, before the queue hears it.
  afterPublishCommit('after-publish-commit');

  const KillPoint(this.label);

  /// The point as `CLAX_TEST_KILL_AT` names it.
  final String label;

  static KillPoint? parse(String? s) {
    for (final p in values) {
      if (p.label == s) return p;
    }
    return null;
  }
}

/// A debug build's hook for the kill-and-resume test
/// (integration_test/kill_resume_test.dart): at [at], the runner writes
/// [file] (the point's name and the job's ID) and waits forever, for the
/// test to kill the process there.
final class KillHook {
  const KillHook(this.at, this.file);

  /// The environment variable naming the point.
  static const variable = 'CLAX_TEST_KILL_AT';

  /// The file written at the point, under the support directory.
  static const fileName = 'kill-point';

  /// The hook `CLAX_TEST_KILL_AT` names, in a debug build only; null in
  /// any other build, or without the variable.
  static KillHook? fromEnvironment(String supportDir) {
    if (!kDebugMode) return null;
    final at = KillPoint.parse(Platform.environment[variable]);
    return at == null ? null : KillHook(at, '$supportDir/$fileName');
  }

  final KillPoint at;
  final String file;

  /// Writes [file] for job [jobId] and never completes.
  Future<Never> hold(JobId jobId) async {
    File(file).writeAsStringSync('${at.label} $jobId\n', flush: true);
    return Completer<Never>().future;
  }
}

/// What the worker tells the UI isolate.
abstract interface class UiPort {
  /// A job ended or stopped while no view shows it.
  void notice(Notice notice);

  /// Whether the app is to keep running for the queue.
  void keepAlive(KeepAlive keepAlive);

  /// What the UI shows of the queue now.
  void publishState(PublishState state);
}

/// [UiPort] over a worker's outlet to the UI isolate: each effect goes as
/// the codec's [UiEffect].
final class _ToUi implements UiPort {
  const _ToUi(this._toUi);
  final void Function(ToUi message) _toUi;
  @override
  void notice(Notice notice) => _toUi(UiEffect(notice));
  @override
  void keepAlive(KeepAlive keepAlive) => _toUi(UiEffect(keepAlive));
  @override
  void publishState(PublishState state) => _toUi(UiEffect(state));
}

/// Runs the queue's effects on the core through [core] (the worker's
/// `CoreTransport`), the agent through [agent], renders through
/// [render], and the UI's news through [ui], and [send]s each answer.
///
/// [start] subscribes to the core's event stream: its `ready` names the
/// stream a claim holds the queue with, and a claim subscribes that stream
/// to the `jobs` topic, whose `job` and `jobs_owner` events become
/// [JobNoticed] and [OwnerChanged]. A `ready` naming another stream than
/// the one before (the stream ended and was opened again) is
/// [OwnerChanged] `held: false`: the claim held with the old stream is
/// released with it.
///
/// Every write of a job carries the owner token the last claim answered
/// in `x-clax-jobs-owner`. A commit answered 2xx is [Committed] with the
/// row after it; any other status is [Refused], with `version_moved`,
/// `doc_moved`, `not_rendered_clean`, `job_moved` (with its row) and
/// `not_owner` as their [Refusal]s and every other code as [Invalid]. A
/// call the core cannot answer (it closed) answers nothing: the core's
/// reopen loses the claim and loads the jobs again. A commit with no
/// answer in its time reads the job back and answers [JobRead] with its
/// row, which says whether the commit landed; a creation asks again,
/// which the core answers once for its ID and body.
///
/// With [importState], each [LoadJobs] first runs it with the claim's
/// token (the app's one-time JSON import, lib/core/json_import.dart),
/// before it reads the jobs, so the jobs it makes load with the rest. Its
/// failure ([Exception]) is logged and the jobs load anyway; the files it
/// left are sent again at the next load.
///
/// Work (a turn, a tool call, a render) is dropped by [cancelJob]: what it
/// answers afterwards is never sent. An [Error] the agent throws is not
/// caught: a bug ends the worker.
class EffectRunner implements WorkerRunner {
  EffectRunner({
    required this.core,
    required this.agent,
    required this.render,
    required this.ui,
    required this.probeClaude,
    required this.session,
    required this.send,
    this.client = 'app',
    this.requestTimeout = const Duration(seconds: 35),
    this.stepTimeout = const Duration(seconds: 125),
    this.kill,
    this.importState,
  });

  /// The runner of the worker [context] belongs to, subscribed to the
  /// core's events at once: its calls go to the worker's core over a
  /// [CoreTransport], its answers to [WorkerContext.send], its renders to
  /// the worker's [WorkerContext.render] (unless [render] is given), and
  /// `Notice` and `KeepAlive` to [WorkerContext.toUi] as the codec's
  /// [UiEffect]s. With [importJson], it runs [importJsonState] on the
  /// worker's support directory before each load of the jobs.
  factory EffectRunner.forWorker(
    WorkerContext context, {
    required AgentPort agent,
    RenderPort? render,
    required Future<bool> Function() probeClaude,
    required Future<String> Function() session,
    bool importJson = false,
  }) => EffectRunner(
    core: CoreTransport(context.core),
    agent: agent,
    render: render ?? context.render,
    ui: _ToUi(context.toUi),
    probeClaude: probeClaude,
    session: session,
    send: context.send,
    kill: KillHook.fromEnvironment(context.start.supportDir),
    importState: importJson
        ? (core, token) => importJsonState(
            supportDir: context.start.supportDir,
            core: core,
            ownerToken: token,
            clock: context.start.clock,
          )
        : null,
  )..start();

  final ClaxTransport core;
  final AgentPort agent;
  final RenderPort render;
  final UiPort ui;

  /// Whether Claude answers (the `ApiConnection` probe).
  final Future<bool> Function() probeClaude;

  /// The worker's live session, which asks the `ask` tool's questions and
  /// is named in every reply step.
  final Future<String> Function() session;

  /// Feeds a result back into the loop.
  final void Function(Msg msg) send;

  /// The queue's client: `app`.
  final String client;

  /// A call's time limit; [stepTimeout] for a step's commit, which may
  /// write a version.
  final Duration requestTimeout;
  final Duration stepTimeout;

  /// A debug build's kill hook, which holds a `Publish` at its point.
  final KillHook? kill;

  /// What runs before each load of the jobs, with [core] and the claim's
  /// token: the app's JSON import, when the worker runs it.
  final Future<ImportSummary?> Function(ClaxTransport core, String token)?
  importState;

  static const ownerHeader = 'x-clax-jobs-owner';

  /// The token the last claim answered.
  String? _token;

  StreamSubscription<SseMessage>? _events;

  /// The stream the core's last `ready` named, and the claims waiting for
  /// a stream.
  String? _stream;
  final List<Completer<String>> _streamWaiters = [];

  /// The work in flight by job, each with the signal that cancels it.
  final Map<JobId, Map<EffectId, Completer<void>>> _work = {};

  bool _closed = false;

  /// Subscribes to the core's events.
  void start() {
    if (_closed || _events != null) return;
    _events = core.sse().listen(
      _onEvent,
      onError: (Object _) => _streamEnded(),
      onDone: _streamEnded,
      cancelOnError: true,
    );
  }

  /// Runs [e]; its answer, if any, is [send]t when it comes.
  @override
  void run(Effect e) {
    if (_closed) return;
    switch (e) {
      case ClaimOwner():
        unawaited(_claim(e));
      case LoadJobs():
        unawaited(_load(e));
      case ReadJob():
        unawaited(_read(e));
      case CreateJob():
        unawaited(_create(e));
      case MoveJob():
        unawaited(
          _commit(e, 'PATCH', _job(e.jobId), {
            'if_rev': e.ifRev,
            'state': e.to.name,
            ..._field('error', e.error, _errorJson),
            ..._field('carry', e.carry, _carryJson),
            ..._field('plan', e.plan, _planJson),
          }),
        );
      case CommitStep():
        unawaited(
          _commit(e, 'POST', _step(e.jobId, e.key), {
            'result': _turnResultJson(e.result),
          }),
        );
      case Publish():
        unawaited(_publish(e));
      case Reply():
        unawaited(_reply(e));
      case Ask():
        unawaited(_ask(e));
      case StartTurn():
        unawaited(_turn(e));
      case RunTool():
        unawaited(_tool(e));
      case Render():
        unawaited(_render(e));
      case StopWork(:final jobId):
        cancelJob(jobId);
        render.cancel(jobId);
        agent.forget(jobId);
      case ProbeClaude():
        unawaited(_probe(e));
      case Notice():
        ui.notice(e);
      case KeepAlive():
        ui.keepAlive(e);
      case PublishState():
        ui.publishState(e);
    }
  }

  /// Abandons job [id]'s work in flight: each is told to stop, and what it
  /// answers is dropped.
  void cancelJob(JobId id) {
    for (final stop in (_work.remove(id) ?? const {}).values) {
      if (!stop.isCompleted) stop.complete();
    }
  }

  /// Stops: work in flight is cancelled, the event stream ends, the claim
  /// is released, and nothing more is sent.
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    for (final id in [..._work.keys]) {
      cancelJob(id);
    }
    for (final w in _streamWaiters) {
      w.completeError(StateError('the runner closed'));
    }
    _streamWaiters.clear();
    await _events?.cancel();
    _events = null;
    if (_token case final token?) {
      _token = null;
      try {
        await core.send(
          'DELETE',
          '/api/jobs/owner',
          headers: {ownerHeader: token},
          timeout: requestTimeout,
        );
      } on ClaxApiError catch (_) {
        // The claim ends with the stream in any case.
      }
    }
  }

  void _send(Msg m) {
    if (_closed) return;
    // A job whose row ended, or that the core no longer holds, has no
    // more work: its agent lets go of it.
    switch (m) {
      case Committed(:final job) || Refused(job: final job?)
          when job.state.isFinal:
        agent.forget(job.id);
      case JobRead(:final jobId, :final job)
          when job == null || job.state.isFinal:
        agent.forget(jobId);
      default:
        break;
    }
    send(m);
  }

  // ------------------------------------------------------------ events

  void _onEvent(SseMessage m) {
    switch (m.event) {
      case 'ready':
        final id = switch (_decode(m.data)) {
          {'stream': final String s} => s,
          _ => null,
        };
        if (id == null) return;
        final before = _stream;
        _stream = id;
        for (final w in _streamWaiters) {
          w.complete(id);
        }
        _streamWaiters.clear();
        if (before != null && before != id) {
          _token = null;
          _send(OwnerChanged(client: client, held: false));
        }
      case 'job':
        if (_decode(m.data) case {
          'id': final String id,
          'client': final String c,
          'state': final String s,
          'rev': final int rev,
        }) {
          final state = JobState.parse(s);
          if (state == null) return;
          final step = (_decode(m.data) as Map)['step'];
          _send(
            JobNoticed(
              id,
              client: c,
              state: state,
              step: step is String ? StepKey.parse(step) : null,
              rev: rev,
            ),
          );
        }
      case 'jobs_owner':
        if (_decode(m.data) case {
          'client': final String c,
          'held': final bool held,
        }) {
          _send(OwnerChanged(client: c, held: held));
        }
    }
  }

  /// The stream ended for good (the core closed, or refused it): the
  /// claim went with it; the next claim subscribes again.
  void _streamEnded() {
    _events = null;
    _stream = null;
    if (_closed) return;
    if (_token != null) {
      _token = null;
      _send(OwnerChanged(client: client, held: false));
    }
  }

  /// The live stream to claim with, other than [not]: waits for the next
  /// `ready` when there is none, subscribing again if the stream ended.
  Future<String> _streamOtherThan(String? not) {
    if (_stream case final s? when s != not) return Future.value(s);
    start();
    final w = Completer<String>();
    _streamWaiters.add(w);
    return w.future;
  }

  // ------------------------------------------------------------ the owner

  Future<void> _claim(ClaimOwner e) async {
    String? refused;
    while (!_closed) {
      final String stream;
      try {
        stream = await _streamOtherThan(refused);
      } on StateError catch (_) {
        return;
      }
      final a = await _call(
        'POST',
        '/api/jobs/owner',
        body: {'client': client, 'stream': stream},
      );
      switch (a) {
        case _Ok(json: {'owner': final String token}):
          _token = token;
          _send(OwnerClaimed(e.id, token));
          return;
        case _Refused(code: 'owner_held'):
          _send(OwnerHeld(e.id));
          return;
        case _Refused(code: 'owner_draining'):
          _send(OwnerDraining(e.id));
          return;
        case _Refused(code: 'no_such_stream'):
          // The stream ended since its `ready`: claim with the next one.
          refused = stream;
        case _Unanswered(timedOut: true):
          // The claim may have landed: the same stream's claim answers
          // its token again.
          break;
        case _Unanswered() || _Ok() || _Refused():
          return;
      }
    }
  }

  // ------------------------------------------------------------ reads

  Future<void> _load(LoadJobs e) async {
    if ((importState, _token) case (final run?, final token?)) {
      try {
        if (await run(core, token) case final s?) {
          developer.log('imported the JSON state: $s', name: 'clax queue');
        }
      } on Exception catch (err) {
        developer.log('the JSON state import failed: $err', name: 'clax queue');
      }
    }
    while (!_closed) {
      final a = await _call(
        'GET',
        '/api/jobs',
        query: {'open': '1', 'client': client},
      );
      switch (a) {
        case _Ok(json: {'jobs': final List rows}):
          final jobs = <Job>[];
          for (final r in rows) {
            if (jobOf(r) case final j?) jobs.add(j);
          }
          _send(JobsLoaded(jobs));
          return;
        case _Unanswered(timedOut: true):
          continue;
        case _Unanswered() || _Ok() || _Refused():
          return;
      }
    }
  }

  Future<void> _read(ReadJob e) async {
    while (!_closed) {
      switch (await _readJob(e.jobId)) {
        case (true, final job):
          _send(JobRead(e.id, e.jobId, job));
          return;
        case (false, _):
          return;
      }
    }
  }

  /// Job [id]'s row: (true, the row) when the core answered, null for a
  /// job it holds none of; (false, null) when it could not answer. A read
  /// with no answer in its time is asked again.
  Future<(bool, Job?)> _readJob(JobId id) async {
    while (!_closed) {
      switch (await _call('GET', _job(id))) {
        case _Ok(json: {'job': final row}):
          return (true, jobOf(row));
        case _Refused(status: 404):
          return (true, null);
        case _Unanswered(timedOut: true):
          continue;
        case _Unanswered() || _Ok() || _Refused():
          return (false, null);
      }
    }
    return (false, null);
  }

  // ------------------------------------------------------------ commits

  Future<void> _create(CreateJob e) => _commit(e, 'POST', '/api/jobs', {
    'id': e.jobId,
    'client': client,
    'kind': e.kind.name,
    'artifact_id': ?e.artifactId,
    'title': ?e.title,
    'input': _inputJson(e.input),
    if (e.plan case final p?) 'plan': _planJson(p),
  });

  /// Sends commit [e] as [method] [path] with [body] and the owner's
  /// token, and answers it.
  Future<void> _commit(
    CommitEffect e,
    String method,
    String path,
    Map<String, Object?> body, {
    bool again = true,
  }) async {
    final a = await _call(
      method,
      path,
      body: body,
      owner: true,
      timeout: e is CreateJob || e is MoveJob || e is Ask
          ? requestTimeout
          : stepTimeout,
    );
    if (_closed) return;
    if (kill case final k?
        when k.at == KillPoint.afterPublishCommit && e is Publish && a is _Ok) {
      await k.hold(e.jobId);
    }
    switch (a) {
      case _Ok(:final json):
        final job = jobOf(json['job']);
        if (job == null) {
          _send(Refused(e.id, e.jobId, const Invalid('malformed')));
          return;
        }
        final key = switch (json['step']) {
          {'key': final String k} => StepKey.parse(k),
          _ => null,
        };
        _send(
          Committed(
            e.id,
            job,
            step: key == null ? null : stepResultOf(key, json['step']),
            replayed: json['replayed'] == true,
          ),
        );
      case _Refused():
        _send(_refusal(e, a));
      case _Unanswered(timedOut: true) when e is CreateJob && again:
        // The core answers the same ID and body once.
        await _commit(e, method, path, body, again: false);
      case _Unanswered(timedOut: true):
        // Whether it landed, the row says.
        if (await _readJob(e.jobId) case (true, final job)) {
          _send(JobRead(e.id, e.jobId, job));
        }
      case _Unanswered():
        break;
    }
  }

  Msg _refusal(CommitEffect e, _Refused a) {
    final Refusal reason = switch (a) {
      _Refused(status: 409, code: 'version_moved', :final error) =>
        VersionMoved(_int(error['current']) ?? 0),
      _Refused(status: 409, code: 'doc_moved', :final error) => DocMoved(
        error['path'] as String? ?? '',
      ),
      _Refused(status: 409, code: 'not_rendered_clean') =>
        const NotRenderedClean(),
      _Refused(status: 409, code: 'job_moved') => const JobMoved(),
      _Refused(status: 409, code: 'not_owner') => const NotOwner(),
      _Refused(:final code, :final message) => Invalid(code, message),
    };
    return Refused(
      e.id,
      e.jobId,
      reason,
      job: reason is JobMoved ? jobOf(a.error['job']) : null,
    );
  }

  /// Commits [e]'s publish step with the plan the agent makes of its draft
  /// now ([AgentPort.publishPlan]), at [e]'s base.
  Future<void> _publish(Publish e) async {
    final PublishPlan plan;
    try {
      plan = await agent.publishPlan(e.jobId, e.turn, e.callId, e.publish);
    } on PublishRefused catch (err) {
      _send(Refused(e.id, e.jobId, Invalid(err.code, err.message)));
      return;
    } on Exception catch (err) {
      _send(Refused(e.id, e.jobId, Invalid('no_draft', '$err')));
      return;
    }
    if (kill case final k? when k.at == KillPoint.beforePublishCommit) {
      await k.hold(e.jobId);
    }
    await _commit(e, 'POST', _step(e.jobId, e.key), {
      'publish': {..._object(plan.data), 'base': e.base},
      if (e.done) 'state': 'done',
    });
  }

  /// Commits [e]'s reply step, ending the change, with the worker's
  /// session as its `session_id`: the core makes the reply from the
  /// session that published the job's version while it is live, and from
  /// this one once it has ended or when the version names none (a change
  /// imported from the app's ledger). When the session cannot be had the
  /// step goes without it, and the core alone decides whether it needs one.
  Future<void> _reply(Reply e) async {
    String? sid;
    try {
      sid = await session();
    } on Exception {
      sid = null;
    }
    if (_closed) return;
    await _commit(e, 'POST', _step(e.jobId, e.key), {
      'reply': switch (e.reply.data) {
        final Map d => {...d.cast<String, Object?>(), 'session_id': ?sid},
        final d => d,
      },
      'state': 'done',
    });
  }

  /// Creates the question from the worker's session with the call's tool
  /// call ID (the core answers the same question for the same session and
  /// tool call), then records it as the job's.
  Future<void> _ask(Ask e) async {
    final String sid;
    try {
      sid = await session();
    } on Exception catch (err) {
      _send(Refused(e.id, e.jobId, Invalid('unknown_session', '$err')));
      return;
    }
    final asked = await _call(
      'POST',
      '/api/sessions/${Uri.encodeComponent(sid)}/questions',
      body: {
        ...switch (e.question.data) {
          final Map d => d.cast<String, Object?>(),
          final d => {'questions': d},
        },
        'source': 'ask',
        'tool_use_id': e.toolUseId,
      },
    );
    switch (asked) {
      case _Ok(json: {'question': {'id': final String qid}}):
        await _commit(
          e,
          'PUT',
          '${_job(e.jobId)}/questions/${Uri.encodeComponent(qid)}',
          const {},
        );
      case _Refused():
        _send(_refusal(e, asked));
      case _Unanswered(timedOut: true):
        // Asked again, the same tool call answers the same question.
        if (await _readJob(e.jobId) case (true, final job)) {
          _send(JobRead(e.id, e.jobId, job));
        }
      case _Ok() || _Unanswered():
        break;
    }
  }

  // ------------------------------------------------------------ work

  Future<void> _turn(StartTurn e) async {
    final stop = _begin(e.jobId, e.id);
    final Msg answer;
    try {
      answer = TurnDone(
        e.id,
        e.jobId,
        await agent.turn(e.jobId, e.input, stop.future),
      );
    } on AgentError catch (err) {
      if (!_end(e.jobId, e.id)) return;
      _send(
        TurnFailed(
          e.id,
          e.jobId,
          err.kind == AgentErrorKind.network
              ? TurnFailure.network
              : TurnFailure.other,
          detail: err.message,
        ),
      );
      return;
    } on Exception catch (err) {
      if (!_end(e.jobId, e.id)) return;
      _send(TurnFailed(e.id, e.jobId, TurnFailure.other, detail: '$err'));
      return;
    }
    if (_end(e.jobId, e.id)) _send(answer);
  }

  Future<void> _tool(RunTool e) async {
    final stop = _begin(e.jobId, e.id);
    ToolResult result;
    try {
      result = await agent.tool(e.jobId, e.turn, e.call, stop.future);
    } on AgentError catch (err) {
      result = ToolResult(data: {'error': err.message}, isError: true);
    } on Exception catch (err) {
      result = ToolResult(data: {'error': '$err'}, isError: true);
    }
    if (_end(e.jobId, e.id)) {
      _send(ToolDone(e.id, e.jobId, e.call.id, result));
    }
  }

  /// Asks the UI isolate to render, records a render's result in the
  /// core under the spec's digest (spec §8.2), then answers [Rendered]; a
  /// render the UI lost is [RenderLost]. A render whose spec has no digest
  /// is not recorded. A record refused or unanswered leaves the draft
  /// unrendered: its publish is refused `not_rendered_clean`, and planned
  /// again.
  Future<void> _render(Render e) async {
    _begin(e.jobId, e.id);
    final RenderSpec spec;
    try {
      spec = await agent.renderSpec(e.jobId, e.turn, e.callId, e.spec);
    } on Exception catch (err) {
      if (_end(e.jobId, e.id)) {
        _send(
          Rendered(
            e.id,
            e.jobId,
            RenderOutcome(
              clean: false,
              data: {
                'error': 'The draft could not be served: $err',
                'errors': ['The draft could not be served: $err'],
                'failed_tests': const <String>[],
              },
            ),
          ),
        );
      }
      return;
    }
    if (!_live(e.jobId, e.id)) return;
    final result = await render.render(
      RenderRequest(e.id, e.jobId, e.callId, spec),
    );
    if (!_live(e.jobId, e.id)) return;
    switch (result) {
      case RenderGone():
        _end(e.jobId, e.id);
        _send(RenderLost(e.id, e.jobId));
      case RenderAnswered(:final outcome):
        if (spec.digest case final digest?) {
          await _call(
            'POST',
            '/api/drafts/${Uri.encodeComponent(spec.draftId)}/renders',
            body: {
              'digest': digest,
              'errors': outcome.errors,
              'failed_tests': outcome.failedTests,
            },
          );
        }
        if (!_live(e.jobId, e.id)) return;
        agent.rendered(e.jobId, e.turn, e.callId, outcome);
        if (_end(e.jobId, e.id)) _send(Rendered(e.id, e.jobId, outcome));
    }
  }

  Future<void> _probe(ProbeClaude e) async {
    bool reachable;
    try {
      reachable = await probeClaude();
    } on Exception catch (_) {
      reachable = false;
    }
    _send(ClaudeReachable(reachable));
  }

  Completer<void> _begin(JobId job, EffectId id) =>
      (_work[job] ??= {})[id] = Completer<void>();

  bool _live(JobId job, EffectId id) => _work[job]?.containsKey(id) ?? false;

  /// Ends work [id] of [job]: whether it was still live (not cancelled).
  bool _end(JobId job, EffectId id) {
    final held = _work[job];
    final live = held?.remove(id) != null;
    if (held != null && held.isEmpty) _work.remove(job);
    return live && !_closed;
  }

  // ------------------------------------------------------------ calls

  Future<_Answer> _call(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, Object?>? body,
    bool owner = false,
    Duration? timeout,
  }) async {
    final TransportAnswer a;
    try {
      a = await core.send(
        method,
        path,
        query: query,
        headers: {
          if (body != null) 'content-type': 'application/json',
          if (owner && _token != null) ownerHeader: _token!,
        },
        body: body == null ? null : Uint8List.fromList(utf8.encode(jsonEncode(body))),
        timeout: timeout ?? requestTimeout,
      );
    } on ClaxApiError catch (e) {
      return _Unanswered(timedOut: e.code == ClaxApiError.timeout);
    }
    final json = a.body.isEmpty ? null : _decode(utf8.decode(a.body, allowMalformed: true));
    if (a.status >= 200 && a.status < 300) {
      return _Ok(json is Map ? json.cast<String, Object?>() : const {});
    }
    final error = switch (json) {
      {'error': final Map e} => e.cast<String, Object?>(),
      _ => const <String, Object?>{},
    };
    return _Refused(
      a.status,
      error['code'] as String? ?? 'http_${a.status}',
      error['message'] as String?,
      error,
    );
  }

  static Object? _decode(String s) {
    try {
      return jsonDecode(s);
    } on FormatException catch (_) {
      return null;
    }
  }

  static String _job(JobId id) => '/api/jobs/${Uri.encodeComponent(id)}';

  /// A step's route: its key as the core spells it (`turn:<n>`,
  /// `publish:<base>`, `reply`).
  static String _step(JobId id, StepKey key) => '${_job(id)}/steps/$key';
}

sealed class _Answer {
  const _Answer();
}

/// 2xx, with its JSON object.
final class _Ok extends _Answer {
  const _Ok(this.json);
  final Map<String, Object?> json;
}

/// Any other status, with the core's error object.
final class _Refused extends _Answer {
  const _Refused(this.status, this.code, this.message, this.error);
  final int status;
  final String code;
  final String? message;
  final Map<String, Object?> error;
}

/// No answer: none came within the call's time ([timedOut]), so whether
/// it took effect is unknown, or none can come.
final class _Unanswered extends _Answer {
  const _Unanswered({required this.timedOut});
  final bool timedOut;
}

// ------------------------------------------------------------ rows

/// The queue's [Job] of a row as the core's job routes answer it
/// (`{job, steps, threads, questions}`); null for one the queue cannot
/// read (another kind, or a state the core does not have).
Job? jobOf(Object? full) {
  if (full is! Map) return null;
  final row = full['job'];
  if (row is! Map) return null;
  final kind = switch (row['kind']) {
    'make' => JobKind.make,
    'change' => JobKind.change,
    _ => null,
  };
  final state = switch (row['state']) {
    final String s => JobState.parse(s),
    _ => null,
  };
  final id = row['id'];
  final rev = _int(row['rev']);
  if (kind == null || state == null || id is! String || rev == null) {
    return null;
  }
  final steps = <StepKey, StepResult>{};
  for (final s in (full['steps'] as List?) ?? const []) {
    if (s case {'key': final String k}) {
      final key = StepKey.parse(k);
      steps[key] = stepResultOf(key, s);
    }
  }
  return Job(
    id: id,
    kind: kind,
    state: state,
    step: switch (row['step']) {
      final String s => StepKey.parse(s),
      _ => null,
    },
    rev: rev,
    artifactId: row['artifact_id'] as String?,
    title: row['title'] as String?,
    input: _inputOf(row['input']),
    plan: switch (row['plan']) {
      final Map p => PlanState(
        planRest: p['plan_rest'] == true,
        data: p['data'],
      ),
      _ => null,
    },
    carry: switch (row['carry']) {
      final Map c => Carry(
        stops: _int(c['stops']) ?? 0,
        canKeepGoing: c['can_keep_going'] == true,
        data: c['data'],
      ),
      _ => null,
    },
    error: switch (row['error']) {
      {'code': final String code} && final Map e => JobError(
        code,
        e['detail'],
        switch (e['step']) {
          final String s => StepKey.parse(s),
          _ => null,
        },
      ),
      _ => null,
    },
    steps: steps,
    threads: {
      for (final t in (full['threads'] as List?) ?? const [])
        if (t case {
          'thread_id': final String tid,
          'comment_id': final String cid,
        })
          tid: (cid, t['stopped'] == true),
    },
    questions: {
      for (final q in (full['questions'] as List?) ?? const [])
        if (q case {'question_id': final String qid})
          qid: JobQuestion(
            _answerOf(q['answer']),
            toolUseId: q['tool_use_id'] as String?,
          ),
    },
  );
}

/// A committed step's result (`{key, result, version_n}`), by the kind its
/// [key] names.
StepResult stepResultOf(StepKey key, Object? step) {
  final result = step is Map ? step['result'] : null;
  return switch (key) {
    TurnKey() => switch (result) {
      final Map r => TurnStepResult(
        _notesOf(r['notes']),
        base: _int(r['base']) ?? 0,
      ),
      _ => const TurnStepResult(TurnNotes()),
    },
    PublishKey() => PublishStepResult(
      artifactId: (result is Map ? result['artifact_id'] : null) as String? ?? '',
      version:
          _int(result is Map ? result['version'] : null) ??
          _int(step is Map ? step['version_n'] : null) ??
          0,
    ),
    ReplyKey() || UnknownKey() => ReplyStepResult(data: result),
  };
}

JobInput _inputOf(Object? v) => switch (v) {
  final Map i => JobInput(
    request: i['request'] as String? ?? '',
    base: _int(i['base']) ?? 0,
    threads: {
      for (final e in ((i['threads'] as Map?) ?? const {}).entries)
        if (e.value is String) e.key as String: e.value as String,
    },
  ),
  _ => const JobInput(request: ''),
};

Map<String, Object?> _inputJson(JobInput i) => {
  'request': i.request,
  'base': i.base,
  'threads': i.threads,
};

Map<String, Object?> _planJson(PlanState p) => {
  'plan_rest': p.planRest,
  'data': p.data,
};

Map<String, Object?> _carryJson(Carry c) => {
  'stops': c.stops,
  'can_keep_going': c.canKeepGoing,
  'data': c.data,
};

Map<String, Object?> _errorJson(JobError e) => {
  'code': e.code,
  'detail': e.detail,
  'step': e.step?.toString(),
};

/// A patch's field: absent to keep it, `null` to clear it.
Map<String, Object?> _field<T>(
  String name,
  Field<T> f,
  Object? Function(T) json,
) => switch (f) {
  Keep() => const {},
  Clear() => {name: null},
  SetTo(:final value) => {name: json(value)},
};

/// Step `turn:<n>`'s result: the turn's notes and the version it was
/// planned on.
Map<String, Object?> _turnResultJson(TurnStepResult r) => {
  'base': r.base,
  'notes': {
    'calls': [for (final c in r.notes.calls) _callJson(c)],
    'data': r.notes.data,
  },
};

Map<String, Object?> _callJson(ToolCall c) => switch (c) {
  FinishCall(:final id, :final publish, :final reply) => {
    'id': id,
    'tool': 'finish',
    'publish': {'base': publish.base, 'data': publish.data},
    if (reply != null) 'reply': reply.data,
  },
  RenderCall(:final id, :final spec) => {
    'id': id,
    'tool': 'render',
    'draft_id': spec.draftId,
    'revision': spec.revision,
    'kind': spec.kind.name,
    'digest': spec.digest,
    'files': spec.files,
    if (spec.migration case final m?)
      'migration': {'file': m.file, 'from': m.from, 'to': m.to},
  },
  AskCall(:final id, :final question) => {
    'id': id,
    'tool': 'ask',
    'question': question.data,
  },
  OtherCall(:final id, :final name, :final args) => {
    'id': id,
    'tool': name,
    'args': args,
  },
};

TurnNotes _notesOf(Object? v) {
  if (v is! Map) return const TurnNotes();
  return TurnNotes(
    calls: [
      for (final c in (v['calls'] as List?) ?? const [])
        ?_callOf(c),
    ],
    data: v['data'],
  );
}

ToolCall? _callOf(Object? c) {
  if (c is! Map) return null;
  final id = c['id'];
  final tool = c['tool'];
  if (id is! String || tool is! String) return null;
  return switch (tool) {
    'finish' => FinishCall(
      id,
      switch (c['publish']) {
        final Map p => PublishPlan(base: _int(p['base']) ?? 0, data: p['data']),
        _ => const PublishPlan(base: 0),
      },
      reply: c.containsKey('reply') ? ReplyPlan(data: c['reply']) : null,
    ),
    'render' => RenderCall(
      id,
      RenderSpec(
        draftId: c['draft_id'] as String? ?? '',
        revision: _int(c['revision']) ?? 0,
        kind:
            RenderKind.values.where((k) => k.name == c['kind']).firstOrNull ??
            RenderKind.inspect,
        digest: c['digest'] as String?,
        files: [
          for (final f in (c['files'] as List?) ?? const [])
            if (f is String) f,
        ],
        migration: switch (c['migration']) {
          {'file': final String file} && final Map m => MigrationSpec(
            file: file,
            from: _int(m['from']) ?? 0,
            to: _int(m['to']) ?? 0,
          ),
          _ => null,
        },
      ),
    ),
    'ask' => AskCall(id, Question(data: c['question'])),
    _ => OtherCall(id, tool, args: c['args']),
  };
}

/// A question's outcome as the job's row keeps it.
JobAnswer _answerOf(Object? a) {
  if (a is! Map) return const AnswerPending();
  return switch (a['outcome']) {
    'answered' => Answered(a['answers']),
    'declined' => const Declined(),
    'withdrawn' => const Withdrawn(),
    _ => const AnswerPending(),
  };
}

Map<String, Object?> _object(Object? v) =>
    v is Map ? v.cast<String, Object?>() : const {};

int? _int(Object? v) => v is num ? v.toInt() : null;
