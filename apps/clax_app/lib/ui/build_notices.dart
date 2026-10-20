import 'dart:async';

import '../agent/plan.dart';
import '../agent/worker.dart';
import '../platform/clock.dart';
import '../platform/background_work.dart';
import 'build_plan.dart';
import 'kit/verbs.dart';

/// Shows each going build as a background run ("Building Budget · 3/7" and
/// the step), and ends it with news when it is done, capped or failed.
///
/// A build's notification shows its first state at once, then at most one
/// update each [every] on [clock], the latest state winning. The counts come
/// from the plan as last watched in [plans], and the title from [title],
/// asked once per app. A built app's notification says how to open it in
/// the words of the pointer in use, as [verbs] gives them when it ends.
class BuildNotices {
  BuildNotices({
    required this.runs,
    required this.plans,
    required this.title,
    this.clock = const SystemClock(),
    this.every = const Duration(seconds: 1),
    this.verbs = _touch,
  });

  final BackgroundRuns runs;
  final PlanStore plans;
  final Clock clock;
  final Duration every;
  final ClaxVerbs Function() verbs;

  static ClaxVerbs _touch() => ClaxVerbs.touch;

  /// App [aid]'s title, or [fallback] when it cannot be told.
  final Future<String> Function(String aid, {required String fallback}) title;

  final _runs = <String, BackgroundRun>{};
  final _titles = <String, String>{};
  final _plans = <String, Plan?>{};
  final _subs = <String, StreamSubscription<Plan?>>{};
  final _latest = <String, BuildState>{};
  final _shown = <String, BuildState>{};
  final _waits = <String>{};

  /// Takes a step of a build.
  void onBuild(BuildState s) {
    final aid = s.appId;
    if (buildGoing(s)) {
      if (!_runs.containsKey(aid)) {
        _runs[aid] = runs.begin(
          'Building your app',
          'Up next',
          artifactId: aid,
        );
        _subs[aid] = plans.watch(aid).listen((p) => _plans[aid] = p);
        if (!_titles.containsKey(aid)) {
          unawaited(
            title(aid, fallback: 'your app').then((t) {
              _titles[aid] = t;
              _show(aid);
            }),
          );
        }
      }
      _latest[aid] = s;
      _throttle(aid);
      return;
    }
    _latest.remove(aid);
    _shown.remove(aid);
    _plans.remove(aid);
    unawaited(_subs.remove(aid)?.cancel());
    final run = _runs.remove(aid);
    if (run == null) return;
    unawaited(
      title(aid, fallback: 'Your app').then(
        (t) => runs.end(run, switch (s.phase) {
          BuildPhase.done => RunReady(
            aid,
            '$t is built',
            '${verbs().tap} to open it',
          ),
          BuildPhase.capped => RunReady(
            aid,
            '$t: build paused',
            'It reached the build’s spending cap',
          ),
          BuildPhase.failed => RunFailed(
            'Couldn’t finish building $t',
            s.reason ?? 'The build stopped',
            artifactId: aid,
          ),
          _ => const RunQuiet(),
        }),
      ),
    );
  }

  /// Stops watching the plans.
  void dispose() {
    for (final s in _subs.values) {
      unawaited(s.cancel());
    }
    _subs.clear();
  }

  void _throttle(String aid) {
    if (!_waits.add(aid)) return;
    _show(aid);
    unawaited(
      clock.delay(every).then((_) {
        _waits.remove(aid);
        final latest = _latest[aid];
        if (latest != null && !identical(latest, _shown[aid])) _throttle(aid);
      }),
    );
  }

  void _show(String aid) {
    final run = _runs[aid];
    final s = _latest[aid];
    if (run == null || s == null) return;
    _shown[aid] = s;
    final known = _titles[aid];
    final name = known ?? 'your app';
    final plan = _plans[aid];
    final count = plan == null ? null : planCount(plan);
    final feature = buildFeature(plan, s);
    runs.step(
      run,
      buildStep(s.progress) ??
          (feature == null ? 'Up next' : 'Building ${feature.title}'),
      title: count == null || count.total == 0
          ? 'Building $name'
          : 'Building $name · ${count.done}/${count.total}',
      app: known,
    );
  }
}
