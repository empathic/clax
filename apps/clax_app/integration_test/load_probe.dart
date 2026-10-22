/// What a load costs the UI isolate and the frames, for the load timing
/// test (`load_timing_test.dart`).
library;

import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show FramePhase, FrameTiming;

import 'package:flutter/scheduler.dart';

/// Watches the UI isolate while a load runs:
///
/// - **Event-loop lag**: a timer every [period] notes how late it fires;
///   a turn of the loop held by other work makes it late. [busyMs] sums
///   each tick's lateness past [period], [maxStallMs] is the longest gap.
/// - **Frames**: a ticker asks for a frame at every vsync, as the cover's
///   animated mark does, and the engine's [FrameTiming]s say how each went:
///   [frames] produced, [slowFrames] whose build or raster took more than
///   a frame's time, and [missedVsyncs], the vsyncs between two frames
///   that produced none.
class LoadProbe {
  LoadProbe({this.period = const Duration(milliseconds: 4)});

  final Duration period;
  final _clock = Stopwatch();
  Timer? _timer;
  Ticker? _ticker;
  int _last = 0;
  int _busyUs = 0;
  int _maxGapUs = 0;
  final _timings = <FrameTiming>[];

  void start() {
    _clock
      ..reset()
      ..start();
    _last = 0;
    _timer = Timer.periodic(period, (_) {
      final now = _clock.elapsedMicroseconds;
      final gap = now - _last;
      _last = now;
      _maxGapUs = math.max(_maxGapUs, gap);
      final late = gap - period.inMicroseconds;
      if (late > 0) _busyUs += late;
    });
    _ticker = Ticker((_) {})..start();
    SchedulerBinding.instance.addTimingsCallback(_onTimings);
  }

  void _onTimings(List<FrameTiming> t) => _timings.addAll(t);

  /// Stops the probe and says what it saw over [ms] of wall time.
  Map<String, Object?> stop() {
    _timer?.cancel();
    _ticker?.dispose();
    SchedulerBinding.instance.removeTimingsCallback(_onTimings);
    final wall = _clock.elapsedMicroseconds;
    _clock.stop();
    const frameUs = 16667;
    var slow = 0;
    var missed = 0;
    int? lastVsync;
    final buildMs = <double>[];
    for (final t in _timings) {
      final build = t.buildDuration.inMicroseconds;
      final raster = t.rasterDuration.inMicroseconds;
      buildMs.add(build / 1000);
      if (build > frameUs || raster > frameUs) slow++;
      final v = t.timestampInMicroseconds(FramePhase.vsyncStart);
      if (lastVsync != null) {
        final gaps = ((v - lastVsync) / frameUs).round() - 1;
        if (gaps > 0) missed += gaps;
      }
      lastVsync = v;
    }
    buildMs.sort();
    return {
      'wallMs': wall / 1000,
      'busyMs': _busyUs / 1000,
      'maxStallMs': _maxGapUs / 1000,
      'frames': _timings.length,
      'slowFrames': slow,
      'missedVsyncs': missed,
      'buildP50Ms': _at(buildMs, 0.5),
      'buildMaxMs': buildMs.isEmpty ? null : buildMs.last,
    };
  }

  static double? _at(List<double> sorted, double q) => sorted.isEmpty
      ? null
      : sorted[math.min(sorted.length - 1, (sorted.length * q).floor())];
}

/// The scheme's request trace (`TestChannel`'s `schemeTrace`), summed:
/// how many requests, who answered them, and how long WebKit waited for
/// each (its ask to its answer).
Map<String, Object?> summarizeTrace(List<Map<String, Object?>> trace) {
  final ms = <double>[];
  final by = <String, int>{};
  var bytes = 0;
  double? first, last;
  for (final r in trace) {
    final s = (r['start']! as num).toDouble();
    final e = (r['end']! as num).toDouble();
    ms.add((e - s) * 1000);
    by.update(r['by']! as String, (n) => n + 1, ifAbsent: () => 1);
    bytes += (r['bytes']! as num).toInt();
    first = first == null ? s : math.min(first, s);
    last = last == null ? e : math.max(last, e);
  }
  ms.sort();
  return {
    'requests': trace.length,
    'by': by,
    'bytes': bytes,
    'spanMs': first == null ? null : (last! - first) * 1000,
    'sumMs': ms.fold<double>(0, (a, b) => a + b),
    'p50Ms': LoadProbe._at(ms, 0.5),
    'p95Ms': LoadProbe._at(ms, 0.95),
    'maxMs': ms.isEmpty ? null : ms.last,
    'each': [
      for (final r in trace)
        '${r['by']} ${r['status']} ${r['bytes']}B '
            '${(((r['end']! as num) - (r['start']! as num)) * 1000).toStringAsFixed(1)}ms '
            '${r['url']}',
    ],
  };
}
