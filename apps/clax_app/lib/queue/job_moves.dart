/// The core's job states and the one table of how a job moves between
/// them, as the store keeps it (`clax_store::jobs::JobState` and
/// `jobs::may_move`, spec 2026-10-19-in-process-core-design §8.1). The
/// queue never plans a move this table refuses; both tables are checked
/// against `crates/clax-store/tests/fixtures/job-moves.json`.
library;

/// A job's state, as the core's row holds it.
enum JobState {
  /// Created, not yet started.
  queued,
  running,

  /// Holds a question it asked that is not yet closed.
  waiting,

  /// The operating system's time ran out.
  paused,

  /// At a limit; Keep going is offered.
  stopped,

  /// Retry is offered.
  failed,

  /// Final.
  cancelled,

  /// Final.
  done;

  /// Whether no write moves the job out of it.
  bool get isFinal => this == cancelled || this == done;

  /// The state the core's text names; null for none.
  static JobState? parse(String s) {
    for (final v in values) {
      if (v.name == s) return v;
    }
    return null;
  }
}

/// What moves a job.
sealed class JobMove {
  const JobMove();
}

/// A patch naming [state] (`PATCH /api/jobs/{id}`), or the core's cancel
/// when the job's artifact goes.
final class MoveTo extends JobMove {
  const MoveTo(this.state);
  final JobState state;

  @override
  bool operator ==(Object other) => other is MoveTo && other.state == state;
  @override
  int get hashCode => state.hashCode;
  @override
  String toString() => 'to:${state.name}';
}

/// The job records a question it asked.
final class MoveAsk extends JobMove {
  const MoveAsk();
  @override
  bool operator ==(Object other) => other is MoveAsk;
  @override
  int get hashCode => 1;
  @override
  String toString() => 'ask';
}

/// A question the job asked closed.
final class MoveClose extends JobMove {
  const MoveClose();
  @override
  bool operator ==(Object other) => other is MoveClose;
  @override
  int get hashCode => 2;
  @override
  String toString() => 'close';
}

/// The job commits a step, its last when [done].
final class MoveStep extends JobMove {
  const MoveStep({required this.done});
  final bool done;
  @override
  bool operator ==(Object other) => other is MoveStep && other.done == done;
  @override
  int get hashCode => done ? 3 : 4;
  @override
  String toString() => done ? 'step:done' : 'step';
}

/// The state a job in [from] is in after [by], given whether it then
/// [waits] on a question it asked that is not closed; null when [by] may
/// not happen to a job in [from]:
///
/// - `cancelled` and `done` are final: nothing moves a job out of them.
/// - `waiting` is entered only by asking, and only from `running`.
/// - A move to `running` lands in `waiting` while the job waits, and only
///   a question's close takes a waiting job back to `running`.
/// - A patch moves a job to `running`, `paused`, `stopped`, `failed` or
///   `cancelled`, or names the state it is in; never to `queued`,
///   `waiting` or `done`.
/// - A step moves nothing, unless it is the job's last (`done`).
JobState? mayMove(JobState from, JobMove by, {required bool waits}) {
  final run = waits ? JobState.waiting : JobState.running;
  if (from.isFinal) return null;
  return switch (by) {
    MoveTo(:final state) when state == from => from,
    MoveTo(state: JobState.running) => run,
    MoveTo(
      :final state,
    ) when const {
          JobState.paused,
          JobState.stopped,
          JobState.failed,
          JobState.cancelled,
        }.contains(state) =>
      state,
    MoveTo() => null,
    MoveAsk() => from == JobState.running ? run : from,
    MoveClose() => from == JobState.waiting ? run : from,
    MoveStep(done: true) => JobState.done,
    MoveStep(done: false) => from,
  };
}
