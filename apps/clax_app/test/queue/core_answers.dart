import 'dart:collection';
import 'dart:math';

import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/update.dart';

/// The core's jobs as the store keeps them (crates/clax-store/src/jobs.rs),
/// in memory: each commit effect is answered as the job routes answer it,
/// with the row after it (`rev` one on) or a refusal, every move by
/// [mayMove]'s table (which job_moves_test.dart checks against the
/// store's), and every committed write published as a `JobNoticed`.
class FakeJobsCore {
  FakeJobsCore() : rows = {}, versions = {}, questions = {};

  FakeJobsCore._copy(FakeJobsCore o)
    : rows = Map.of(o.rows),
      versions = Map.of(o.versions),
      questions = Map.of(o.questions),
      published = o.published,
      replies = o.replies,
      writes = o.writes;

  /// The job rows, oldest first.
  final Map<JobId, Job> rows;

  /// Each artifact's current version.
  final Map<String, int> versions;

  /// Each question's state: pending while open.
  final Map<String, JobAnswer> questions;

  /// Versions the publish steps made, and reply steps committed.
  int published = 0;
  int replies = 0;

  /// Committed writes of jobs, in order.
  int writes = 0;

  /// Events not yet delivered to a worker.
  final List<JobNoticed> events = [];

  /// A refusal to answer a publish step with before its base is checked
  /// (a moved document, an unrendered draft, a bad icon), or null.
  Refusal? Function(Publish p)? refusePublish;

  /// Called after each committed write, for a test to keep the core as it
  /// was then.
  void Function(FakeJobsCore core)? onWrite;

  /// Whether the worker's claim is gone: every write answers `not_owner`.
  bool ownerLost = false;

  /// The core as it is now, apart from its events and hooks.
  FakeJobsCore copy() => FakeJobsCore._copy(this);

  /// `GET /api/jobs?open=1&client=app`.
  List<Job> open() => [
    for (final j in rows.values)
      if (!j.state.isFinal) j,
  ];

  /// `GET /api/jobs/{id}`.
  Job? read(JobId id) => rows[id];

  /// A question's ID as the runner makes it: one per job, turn and call,
  /// so asking again after a restart finds it.
  static String questionId(Ask a) => 'q:${a.jobId}:${a.turn}:${a.callId}';

  Job _write(Job row) {
    final next = row.copyWith(rev: row.rev + 1);
    rows[row.id] = next;
    writes++;
    events.add(
      JobNoticed(next.id, state: next.state, step: next.step, rev: next.rev),
    );
    onWrite?.call(this);
    return next;
  }

  /// A final move withdraws each question the job asked that is open.
  Job _ending(Job row) {
    if (!row.state.isFinal) return row;
    final qs = {...row.questions};
    for (final e in qs.entries.toList()) {
      if (e.value is AnswerPending) {
        qs[e.key] = const Withdrawn();
        questions[e.key] = const Withdrawn();
      }
    }
    return row.copyWith(questions: qs);
  }

  Msg answer(CommitEffect e) {
    if (ownerLost) return Refused(e.id, e.jobId, const NotOwner());
    final row = rows[e.jobId];
    if (e is CreateJob) return _create(e, row);
    if (row == null) return Refused(e.id, e.jobId, const Invalid('not_found'));
    switch (e) {
      case MoveJob():
        if (row.rev != e.ifRev || row.state.isFinal) {
          return Refused(e.id, e.jobId, const JobMoved(), job: row);
        }
        final to = mayMove(row.state, MoveTo(e.to), waits: row.waits);
        if (to == null) {
          return Refused(e.id, e.jobId, const Invalid('invalid_move'));
        }
        final moved = _ending(
          row.copyWith(
            state: to,
            error: e.error._fn,
            carry: e.carry._fn,
            plan: e.plan._fn,
          ),
        );
        return Committed(e.id, _write(moved));
      case CommitStep():
        return _step(e, row, e.key, TurnStepResult(e.notes), done: false);
      case Ask():
        if (row.state.isFinal) {
          return Refused(e.id, e.jobId, const JobMoved(), job: row);
        }
        final qid = questionId(e);
        if (row.questions.containsKey(qid)) {
          return Committed(e.id, row, questionId: qid);
        }
        final status = questions.putIfAbsent(qid, () => const AnswerPending());
        final qs = {...row.questions, qid: status};
        final waits = qs.values.any((a) => a is AnswerPending);
        final to = mayMove(row.state, const MoveAsk(), waits: waits)!;
        return Committed(
          e.id,
          _write(row.copyWith(state: to, questions: qs)),
          questionId: qid,
        );
      case Publish():
        if (row.steps[e.key] case final s?) {
          return Committed(e.id, row, step: s, replayed: true);
        }
        if (row.state.isFinal) {
          return Refused(e.id, e.jobId, const JobMoved(), job: row);
        }
        final aid = row.artifactId ?? 'app-${row.id}';
        final current = versions[aid] ?? 0;
        if (current != e.base) {
          return Refused(e.id, e.jobId, VersionMoved(current));
        }
        if (refusePublish?.call(e) case final r?) {
          return Refused(e.id, e.jobId, r);
        }
        versions[aid] = e.base + 1;
        published++;
        final step = PublishStepResult(artifactId: aid, version: e.base + 1);
        final moved = _ending(
          row.copyWith(
            steps: {...row.steps, e.key: step},
            step: SetTo(e.key),
            artifactId: SetTo(aid),
            state: mayMove(row.state, MoveStep(done: e.done), waits: row.waits),
          ),
        );
        return Committed(e.id, _write(moved), step: step);
      case Reply():
        if (row.steps[e.key] case final s?) {
          return Committed(e.id, row, step: s, replayed: true);
        }
        if (row.state.isFinal) {
          return Refused(e.id, e.jobId, const JobMoved(), job: row);
        }
        replies++;
        return _step(e, row, e.key, const ReplyStepResult(), done: true);
      case CreateJob():
        throw StateError('handled above');
    }
  }

  Msg _create(CreateJob e, Job? row) {
    if (row != null) {
      final same =
          row.kind == e.kind &&
          row.artifactId == e.artifactId &&
          row.title == e.title &&
          row.input == e.input &&
          row.plan == e.plan;
      return same
          ? Committed(e.id, row)
          : Refused(e.id, e.jobId, const Invalid('job_exists'));
    }
    if (e.artifactId case final aid? when !versions.containsKey(aid)) {
      return Refused(e.id, e.jobId, const Invalid('invalid_artifact'));
    }
    final created = Job(
      id: e.jobId,
      kind: e.kind,
      state: JobState.queued,
      rev: 0,
      artifactId: e.artifactId,
      title: e.title,
      input: e.input,
      plan: e.plan,
    );
    return Committed(e.id, _write(created));
  }

  Msg _step(
    CommitEffect e,
    Job row,
    String key,
    StepResult result, {
    required bool done,
  }) {
    if (row.steps[key] case final s?) {
      return Committed(e.id, row, step: s, replayed: true);
    }
    if (row.state.isFinal) {
      return Refused(e.id, e.jobId, const JobMoved(), job: row);
    }
    final moved = _ending(
      row.copyWith(
        steps: {...row.steps, key: result},
        step: SetTo(key),
        state: mayMove(row.state, MoveStep(done: done), waits: row.waits),
      ),
    );
    return Committed(e.id, _write(moved), step: result);
  }

  /// Question [qid] closes with [a] (an answer, a decline, a withdrawal):
  /// its job stores the outcome and goes back to running once it waits on
  /// none.
  void closeQuestion(String qid, JobAnswer a) {
    if (questions[qid] is! AnswerPending) return;
    questions[qid] = a;
    final row = rows.values
        .where((j) => j.questions.containsKey(qid))
        .firstOrNull;
    if (row == null || row.state.isFinal) return;
    final qs = {...row.questions, qid: a};
    final waits = qs.values.any((x) => x is AnswerPending);
    final to = mayMove(row.state, const MoveClose(), waits: waits)!;
    _write(row.copyWith(state: to, questions: qs));
  }

  /// Another isolate writes job [id] without moving it (the UI records a
  /// thread it answers): its `rev` moves on.
  void touch(JobId id) {
    final row = rows[id];
    if (row == null || row.state.isFinal) return;
    _write(row.copyWith(threads: {...row.threads, 't${row.rev}': ('c', false)}));
  }

  /// Another writer publishes a version of [aid].
  void publishElsewhere(String aid) =>
      versions[aid] = (versions[aid] ?? 0) + 1;

  /// The open questions.
  List<String> get openQuestions => [
    for (final e in questions.entries)
      if (e.value is AnswerPending) e.key,
  ];
}

extension<T> on Field<T> {
  T? Function()? get _fn => switch (this) {
    Keep() => null,
    SetTo(:final value) => () => value,
    Clear() => () => null,
  };
}

/// A turn script: what the model answers to [input] for [job].
typedef TurnScript = TurnOutcome Function(Job job, TurnInput input);

/// A make: a write and a render, a question, then the finish.
TurnOutcome makeScript(Job job, TurnInput input) => switch (input.turn) {
  1 => const TurnOutcome(
    TurnNotes(
      calls: [
        OtherCall('w1', 'write_file'),
        RenderCall('r1', RenderSpec(draftId: 'd', revision: 1)),
      ],
    ),
  ),
  2 => const TurnOutcome(
    TurnNotes(calls: [AskCall('a1', Question(data: 'which colour?'))]),
  ),
  _ => TurnOutcome(
    TurnNotes(calls: [FinishCall('f${input.turn}', PublishPlan(base: input.base))]),
  ),
};

/// A change: a write, then the finish with its reply.
TurnOutcome changeScript(Job job, TurnInput input) => switch (input.turn) {
  1 => const TurnOutcome(TurnNotes(calls: [OtherCall('w1', 'write_file')])),
  _ => TurnOutcome(
    TurnNotes(
      calls: [
        FinishCall(
          'f${input.turn}',
          PublishPlan(base: input.base),
          reply: const ReplyPlan(data: 'done'),
        ),
      ],
    ),
  ),
};

TurnOutcome scriptFor(Job job, TurnInput input) => job.kind == JobKind.make
    ? makeScript(job, input)
    : changeScript(job, input);

/// One worker on a core: sends each message through [update], keeps
/// every effect, and answers each effect as the runner would, the commit
/// effects by the [core], in order, until nothing is left to do.
class World {
  World(this.core, {this.script = scriptFor, this.answerQuestions = true});

  final FakeJobsCore core;
  final TurnScript script;

  /// Whether the person answers each question as soon as it is open.
  final bool answerQuestions;
  QueueModel m = const QueueModel();
  final List<Effect> effects = [];
  final Queue<Msg> inbox = Queue();

  void send(Msg msg) {
    final (next, out) = update(m, msg);
    m = next;
    effects.addAll(out);
    for (final e in out) {
      final a = answerOf(e);
      if (a != null) inbox.add(a);
    }
  }

  /// What answers [e], as the runner would send it.
  Msg? answerOf(Effect e) => switch (e) {
    CommitEffect() => core.answer(e),
    ClaimOwner() => const OwnerClaimed('token'),
    LoadJobs() => JobsLoaded(core.open()),
    ReadJob(:final id, :final jobId) => JobRead(id, jobId, core.read(jobId)),
    ProbeClaude() => const ClaudeReachable(true),
    StartTurn(:final id, :final jobId, :final input) => TurnDone(
      id,
      jobId,
      script(core.read(jobId) ?? m.jobs[jobId]!, input),
    ),
    RunTool(:final id, :final jobId, :final call) => ToolDone(
      id,
      jobId,
      call.id,
      ToolResult(data: 'ran ${call.name}'),
    ),
    Render(:final id, :final jobId) => Rendered(
      id,
      jobId,
      const RenderOutcome(clean: true),
    ),
    Notice() || KeepAlive() || PublishState() || StopWork() => null,
  };

  /// Delivers every answer and event until none is left, [until] holds,
  /// or [limit] messages.
  void run({int limit = 2000, bool Function()? until}) {
    var n = 0;
    while (n++ < limit) {
      if (until != null && until()) return;
      if (inbox.isNotEmpty) {
        send(inbox.removeFirst());
      } else if (core.events.isNotEmpty) {
        send(core.events.removeAt(0));
      } else if (answerQuestions && core.openQuestions.isNotEmpty) {
        core.closeQuestion(
          core.openQuestions.first,
          const Answered({'pick': 'blue'}),
        );
      } else {
        return;
      }
    }
    throw StateError('the world did not settle in $limit messages');
  }

  /// Delivers at most [n] answers and events, as a worker killed after
  /// them would have.
  void steps(int n) {
    for (var i = 0; i < n; i++) {
      if (inbox.isNotEmpty) {
        send(inbox.removeFirst());
      } else if (core.events.isNotEmpty) {
        send(core.events.removeAt(0));
      } else {
        return;
      }
    }
  }

  /// Starts the worker: the core opens and the worker claims the queue.
  World start() {
    send(const CoreOpened());
    return this;
  }
}

/// A random generator of the messages a worker may get: answers to the
/// effects it emitted (the commit effects answered by [core] as the store
/// would), the core's events before or after them, the person's intents
/// and questions, and the world's news.
class Generator {
  Generator(this.rnd, this.core);

  final Random rnd;
  final FakeJobsCore core;
  var _ids = 0;
  var _clock = DateTime.utc(2026, 10, 20, 9);

  /// Effects emitted and not yet answered, by ID.
  final Map<EffectId, Effect> open = {};

  /// The claim the core holds for the worker, if any, and the claim each
  /// job effect was emitted under: a write under another is `not_owner`.
  String? liveToken;
  final Map<EffectId, String?> _tokenOf = {};

  /// Answers decided and not yet delivered.
  final List<Msg> answers = [];

  void took(List<Effect> effects, QueueModel m) {
    final token = switch (m.owner) {
      OwnerHeldBy(:final token) => token,
      _ => null,
    };
    for (final e in effects) {
      if (e is CommitEffect) _tokenOf[e.id] = token;
      switch (e) {
        case Notice() || KeepAlive() || PublishState():
          break;
        case StopWork(:final jobId):
          // The runner abandons the work; an answer it already sent may
          // still arrive.
          open.removeWhere((_, w) => w is WorkEffect && w.jobId == jobId);
        default:
          open[e.id] = e;
      }
    }
  }

  T _pick<T>(List<T> xs) => xs[rnd.nextInt(xs.length)];

  /// The answer to [e], drawn as the runner and the core could give it.
  Msg? _answer(Effect e) {
    switch (e) {
      case CommitEffect():
        if (e is Publish && rnd.nextInt(6) == 0) {
          core.refusePublish = (_) => _pick(const [
            DocMoved('notes.json'),
            NotRenderedClean(),
            Invalid('invalid_icon'),
          ]);
        }
        // The core lost the claim now and then (a reopened core).
        if (rnd.nextInt(40) == 0) liveToken = null;
        core.ownerLost = _tokenOf.remove(e.id) != liveToken || liveToken == null;
        final a = core.answer(e);
        core.ownerLost = false;
        core.refusePublish = null;
        return a;
      case ClaimOwner():
        switch (rnd.nextInt(10)) {
          case 0:
            return const OwnerHeld();
          case 1:
            return const OwnerDraining();
          default:
            // The stream that holds the claim gets its token again.
            final token = liveToken ?? 't${_ids++}';
            liveToken = token;
            return OwnerClaimed(token);
        }
      case LoadJobs():
        return JobsLoaded(core.open());
      case ReadJob(:final id, :final jobId):
        return JobRead(id, jobId, core.read(jobId));
      case ProbeClaude():
        return ClaudeReachable(rnd.nextBool());
      case StartTurn(:final id, :final jobId, :final input):
        final job = core.read(jobId);
        if (job == null) return null;
        return switch (rnd.nextInt(12)) {
          0 => TurnFailed(id, jobId, TurnFailure.network),
          1 => TurnFailed(id, jobId, TurnFailure.other),
          2 => TurnDone(
            id,
            jobId,
            TurnOutcome(
              const TurnNotes(),
              stop: TurnStop(canKeepGoing: rnd.nextBool()),
            ),
          ),
          // The finish at once, on the base the turn planned on.
          3 || 4 || 5 || 6 => TurnDone(
            id,
            jobId,
            TurnOutcome(
              TurnNotes(
                calls: [
                  FinishCall(
                    'f${input.turn}',
                    PublishPlan(base: input.base),
                    reply: const ReplyPlan(data: 'done'),
                  ),
                ],
              ),
            ),
          ),
          _ => TurnDone(id, jobId, scriptFor(job, input)),
        };
      case RunTool(:final id, :final jobId, :final call):
        return ToolDone(id, jobId, call.id, const ToolResult(data: 'ok'));
      case Render(:final id, :final jobId):
        return rnd.nextInt(3) == 0
            ? RenderLost(id, jobId)
            : Rendered(id, jobId, RenderOutcome(clean: rnd.nextBool()));
      case Notice() || KeepAlive() || PublishState() || StopWork():
        return null;
    }
  }

  /// The next message: an answer, an event, or something from the world.
  Msg nextMsg(QueueModel m) {
    final roll = rnd.nextInt(100);
    if (roll < 60 && open.isNotEmpty) {
      final e = _pick(open.values.toList());
      open.remove(e.id);
      final a = _answer(e);
      if (a != null) answers.add(a);
      // The runner sends a committed write again when its answer was lost
      // in transport: the core answers the same key replayed.
      if (a is Committed && e is CommitEffect && e is! MoveJob && rnd.nextInt(8) == 0) {
        answers.add(core.answer(e));
      }
    }
    if (roll < 80 && answers.isNotEmpty) {
      return answers.removeAt(rnd.nextInt(answers.length));
    }
    if (roll < 88 && core.events.isNotEmpty) {
      return core.events.removeAt(rnd.nextInt(core.events.length));
    }
    switch (rnd.nextInt(40)) {
      case < 6:
        return Make('m${_ids++}', 'an app');
      case < 9:
        final apps = core.versions.keys.toList();
        if (apps.isEmpty) return Make('m${_ids++}', 'another app');
        final aid = _pick(apps);
        return Change(
          'c${_ids++}',
          aid,
          base: core.versions[aid]!,
          threads: const {'t1': 'c1'},
        );
      case < 12:
        final ids = core.rows.keys.toList();
        if (ids.isEmpty) return const CanRender(true);
        final id = _pick(ids);
        return _pick([Cancel(id), Retry(id), KeepGoing(id), Discard(id)]);
      case < 14:
        final ids = core.rows.keys.toList();
        if (ids.isNotEmpty) core.touch(_pick(ids));
        return const CanRender(true);
      case < 15:
        final qs = core.openQuestions;
        if (qs.isNotEmpty) {
          core.closeQuestion(
            _pick(qs),
            _pick(const [Answered('yes'), Declined(), Withdrawn()]),
          );
        }
        return const CanRender(true);
      case < 17:
        // Another writer publishes: the next publish step finds its base
        // moved.
        final apps = core.versions.keys.toList();
        if (apps.isNotEmpty) core.publishElsewhere(_pick(apps));
        return const Woke(foreground: true);
      case < 19:
        return CanRender(rnd.nextBool());
      case < 21:
        return ClaudeReachable(rnd.nextBool());
      case < 25:
        _clock = _clock.add(Duration(seconds: rnd.nextInt(90)));
        return Tick(_clock);
      case < 27:
        return Woke(foreground: rnd.nextBool());
      case < 28:
        return OwnerChanged(held: rnd.nextBool());
      case < 29:
        return const OutOfTime();
      case < 30:
        liveToken = null;
        return const CoreOpened();
      default:
        _clock = _clock.add(const Duration(seconds: 5));
        return Tick(_clock);
    }
  }
}

// ------------------------------------------------------------ builders

/// A model whose worker holds the queue ([owned]) with the jobs loaded.
QueueModel model({
  bool owned = true,
  List<Job> jobs = const [],
  List<Effect> inFlight = const [],
  Map<JobId, JobLocal> local = const {},
  Reachability claude = Reachability.reachable,
  bool canRender = true,
  DateTime? now,
}) => QueueModel(
  owner: owned ? const OwnerHeldBy('token') : const OwnerNone(),
  loaded: owned,
  jobs: {for (final j in jobs) j.id: j},
  inFlight: {for (final e in inFlight) e.id: e},
  local: local,
  keepAlive: jobs.any(
    (j) => j.state == JobState.running || j.state == JobState.waiting,
  ),
  claude: claude,
  canRender: canRender,
  now: now,
  nextEffect: 100,
);

/// A job row; its `step` is the last of [steps] unless given.
Job job(
  JobId id, {
  JobState state = JobState.running,
  int rev = 1,
  JobKind kind = JobKind.make,
  Map<String, StepResult> steps = const {},
  String? step,
  String? artifactId,
  Carry? carry,
  PlanState? plan,
  JobError? error,
  Map<String, JobAnswer> questions = const {},
  int base = 0,
  String? title,
}) => Job(
  id: id,
  kind: kind,
  state: state,
  rev: rev,
  step: step ?? (steps.isEmpty ? null : steps.keys.last),
  artifactId: artifactId,
  title: title,
  input: JobInput(request: 'a todo app', base: base),
  plan: plan,
  carry: carry,
  error: error,
  steps: steps,
  questions: questions,
);

/// A turn whose only call is a finish planned on [base].
TurnStepResult finishNotes({required int base, ReplyPlan? reply}) =>
    TurnStepResult(
      TurnNotes(calls: [FinishCall('f', PublishPlan(base: base), reply: reply)]),
    );

/// A turn with [calls].
TurnStepResult turnOf(List<ToolCall> calls) =>
    TurnStepResult(TurnNotes(calls: calls));

/// [m] after each of [msgs], and every effect emitted on the way.
(QueueModel, List<Effect>) updateAll(QueueModel m, List<Msg> msgs) {
  final all = <Effect>[];
  for (final msg in msgs) {
    final (next, out) = update(m, msg);
    m = next;
    all.addAll(out);
  }
  return (m, all);
}
