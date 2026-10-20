/// What the queue's worker asks of the platform while jobs run (spec
/// 2026-10-19-in-process-core-design §7.6): its `KeepAlive` holds the
/// app's process, and the platform's word that its time is spent pauses
/// the jobs.
library;

import 'dart:async';

import '../platform/background_work.dart';
import 'effect.dart';
import 'msg.dart';

/// Holds one [BackgroundRun] while the worker says `KeepAlive(true)`: on
/// Android the `dataSync` foreground service with its progress
/// notification (`ChannelBackgroundWork`), on iOS the background task
/// (`IosBackgroundWork`), through [BackgroundRuns], which shares the one
/// service with the runs the app's own screens hold. `KeepAlive(false)`,
/// or [end] when the worker ends, lets it go.
///
/// Where [BackgroundWork.pausesRuns] (iOS), each [BackgroundWork.stopped]
/// (the background time is nearly spent) is told to the worker as
/// [OutOfTime], which pauses every running job (a `MoveJob` to `paused`)
/// until the app is back in front.
///
/// Nothing here, nor in [BackgroundWork], asks the system to wake the app
/// later: a job the OS or a kill interrupts resumes at the next launch.
final class QueueKeepAlive {
  QueueKeepAlive(
    this._runs,
    this._work, {
    required this._tell,
    required this.title,
    required this.step,
  }) {
    if (_work.pausesRuns) {
      _stops = _work.stopped.listen((_) => _tell(const OutOfTime()));
    }
  }

  final BackgroundRuns _runs;
  final BackgroundWork _work;
  final void Function(Msg news) _tell;

  /// What the held run's notification says until a step replaces it.
  final String title;
  final String step;

  StreamSubscription<void>? _stops;
  BackgroundRun? _run;

  /// The run held for the worker, while it holds one.
  BackgroundRun? get run => _run;

  /// Applies the worker's [keepAlive]: on, the run is begun (once, however
  /// many times the worker says it), with a first ask for permission to
  /// post notifications; off, it ends.
  void apply(KeepAlive keepAlive) {
    if (!keepAlive.on) {
      end();
      return;
    }
    if (_run != null) return;
    unawaited(_work.requestPermission());
    _run = _runs.begin(title, step);
  }

  /// Ends the held run, quietly: the worker's notices tell how jobs ended.
  void end() {
    final run = _run;
    if (run == null) return;
    _run = null;
    _runs.end(run, const RunQuiet());
  }

  /// Ends the held run and stops following the platform's stops.
  void dispose() {
    end();
    unawaited(_stops?.cancel());
    _stops = null;
  }
}
