import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'mark.dart';

/// The Clax mark for a wait with something to show for it (an app's
/// update): the arcs stay still while small dots travel, one after another,
/// from the person's arc (left) to Clax's (right), fading as they go.
///
/// It begins as the resting [ClaxMark], whose middle dot fades as the
/// stream starts. When [finishing] turns true no more dots leave, those in
/// flight fade out, and the middle dot comes back: the resting mark,
/// [finishLength] later, when [onFinished] is called.
///
/// Under reduced motion (`MediaQuery.disableAnimationsOf`) or a quiet
/// [ClaxQuiet] it is the resting [ClaxMark], and a finish ends at once.
class ClaxSynapse extends StatefulWidget {
  const ClaxSynapse({
    super.key,
    this.size = 88,
    this.finishing = false,
    this.onFinished,
    this.seed,
  });

  /// The mark's width; its height is [size] × 16/22.
  final double size;

  /// Whether to play the finish: once, from when it turns true.
  final bool finishing;

  /// Called once a finish has played (at once without motion).
  final VoidCallback? onFinished;

  /// Seeds the spacing of the dots; null for a fresh one.
  final int? seed;

  /// How long a finish plays.
  static const finishLength = Duration(milliseconds: 360);

  @override
  State<ClaxSynapse> createState() => _ClaxSynapseState();
}

class _ClaxSynapseState extends State<ClaxSynapse>
    with SingleTickerProviderStateMixin {
  late final Ticker _ticker = createTicker(_tick);
  late final ClaxSynapseModel _model = ClaxSynapseModel(seed: widget.seed);
  bool _still = false;
  bool _finished = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _still = MediaQuery.disableAnimationsOf(context) || ClaxQuiet.of(context);
    if (_still) {
      _ticker.stop();
      if (widget.finishing) _finish();
    } else if (!_ticker.isActive && !_finished) {
      _ticker.start();
    }
  }

  @override
  void didUpdateWidget(ClaxSynapse old) {
    super.didUpdateWidget(old);
    if (widget.finishing && !old.finishing && _still) _finish();
  }

  void _tick(Duration elapsed) {
    final ms = elapsed.inMicroseconds / 1000;
    if (widget.finishing && _model.finishStart == null) _model.finishAt(ms);
    _model.seek(ms);
    if (_model.finished) {
      _ticker.stop();
      _finish();
    }
  }

  /// Tells [ClaxSynapse.onFinished], once; outside a build.
  void _finish() {
    if (_finished) return;
    _finished = true;
    final phase = SchedulerBinding.instance.schedulerPhase;
    if (phase == SchedulerPhase.persistentCallbacks ||
        phase == SchedulerPhase.midFrameMicrotasks) {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (mounted) widget.onFinished?.call();
      });
    } else {
      widget.onFinished?.call();
    }
  }

  @override
  void dispose() {
    _ticker.dispose();
    _model.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_still) return ClaxMark(size: widget.size);
    final p = claxPalette(context);
    return CustomPaint(
      size: Size(
        widget.size,
        widget.size * ClaxMarkGeometry.height / ClaxMarkGeometry.width,
      ),
      painter: ClaxSynapsePainter(
        model: _model,
        you: p.you,
        agent: p.agent,
        dot: p.fg,
      ),
    );
  }
}

/// The dots' stream over time, in [ClaxMarkGeometry]'s units: where each
/// dot in flight is and how opaque, and the middle dot's opacity, at the
/// time [seek] last set.
///
/// A dot leaves the person's arc every [spacing] ms or so (the gaps drawn
/// from a seeded random source) and crosses to Clax's in [travel] ms;
/// [finishAt] stops the stream and brings the middle dot back.
class ClaxSynapseModel extends ChangeNotifier {
  ClaxSynapseModel({int? seed}) : _random = math.Random(seed);

  /// How long a dot takes to cross, and the gap between dots: [spacing]
  /// give or take [spacingJitter].
  static const double travel = 1100, spacing = 520, spacingJitter = 90;

  /// When the first dot leaves, and how long the middle dot takes to fade.
  static const double firstDot = 120, middleFade = 240;

  /// How long the finish takes.
  static const double finishMs = 360;

  /// A travelling dot's radius.
  static const double radius = 1.05;

  /// Where a dot leaves (inside the person's arc) and lands (inside
  /// Clax's), on the dot's line.
  static const double fromX = 7, toX = 15;

  final math.Random _random;

  /// When each dot still in flight left, oldest first, and when the next
  /// one leaves.
  final List<double> _starts = [];
  double _next = firstDot;

  /// Milliseconds since it began.
  double get time => _t;
  double _t = 0;

  /// When the finish began, or null before it.
  double? get finishStart => _finishStart;
  double? _finishStart;
  double _middleFrom = 0;

  /// Whether the finish has played through.
  bool get finished => _finishStart != null && _t >= _finishStart! + finishMs;

  /// The middle dot's opacity: 1 at rest.
  double middle = 1;

  /// The dots in flight: [dotCount] pairs of x and opacity.
  final Float64List dots = Float64List(2 * _maxDots);
  int dotCount = 0;
  static const _maxDots = 8;

  /// [curve] over [from]…[to] at [t]: 0 before, 1 after.
  static double _span(double t, double from, double to, [Curve? curve]) {
    final x = ((t - from) / (to - from)).clamp(0.0, 1.0);
    return curve == null ? x : curve.transform(x);
  }

  /// Starts the finish at [ms]: no dot leaves after it.
  void finishAt(double ms) {
    seek(ms);
    _finishStart = ms;
    _middleFrom = middle;
  }

  /// Sets the stream to [ms] after it began; time only moves forward.
  void seek(double ms) {
    _t = ms;
    final fs = _finishStart;
    while (_next <= ms && (fs == null || _next <= fs)) {
      _starts.add(_next);
      _next += spacing + (_random.nextDouble() * 2 - 1) * spacingJitter;
    }
    while (_starts.isNotEmpty && _starts.first + travel <= ms) {
      _starts.removeAt(0);
    }
    final e = fs == null ? null : ms - fs;
    // Dots in flight when the finish begins fade out within it.
    final fade = e == null ? 1.0 : 1 - _span(e, 0, finishMs * 0.4);
    var n = 0;
    for (final start in _starts) {
      if (n == _maxDots || start > ms) break;
      final p = (ms - start) / travel;
      dots[2 * n] = fromX + (toX - fromX) * ClaxMotion.standard.transform(p);
      dots[2 * n + 1] = fade * _span(p, 0, 0.15) * (1 - p);
      n++;
    }
    dotCount = n;
    middle = e == null
        ? 1 - _span(ms, 0, middleFade, ClaxMotion.standard)
        : _middleFrom +
              (1 - _middleFrom) * _span(e, 0, finishMs, ClaxMotion.standard);
    if (e != null && e >= finishMs) {
      dotCount = 0;
      middle = 1;
    }
    notifyListeners();
  }
}

/// Paints a [ClaxSynapseModel], repainting as it moves: the arcs still in
/// [you] and [agent], the dots in flight and the middle dot in [dot].
class ClaxSynapsePainter extends CustomPainter {
  ClaxSynapsePainter({
    required this.model,
    required this.you,
    required this.agent,
    required this.dot,
  }) : super(repaint: model);

  final ClaxSynapseModel model;
  final Color you, agent, dot;

  static final Path _you = ClaxMarkGeometry.you();
  static final Path _agent = ClaxMarkGeometry.agent();

  final Paint _stroke = Paint()
    ..style = PaintingStyle.stroke
    ..strokeWidth = ClaxMarkGeometry.stroke
    ..strokeCap = ClaxMarkGeometry.cap
    ..isAntiAlias = true;
  final Paint _fill = Paint()..isAntiAlias = true;

  @override
  void paint(Canvas canvas, Size size) {
    final m = model;
    canvas.save();
    canvas.scale(size.width / ClaxMarkGeometry.width);
    // The dots pass under the arcs: out of the person's, into Clax's.
    for (var i = 0; i < m.dotCount; i++) {
      _fill.color = dot.withValues(alpha: m.dots[2 * i + 1]);
      canvas.drawCircle(
        Offset(m.dots[2 * i], ClaxMarkGeometry.dot.dy),
        ClaxSynapseModel.radius,
        _fill,
      );
    }
    if (m.middle > 0) {
      _fill.color = dot.withValues(alpha: m.middle);
      canvas.drawCircle(
        ClaxMarkGeometry.dot,
        ClaxMarkGeometry.dotRadius,
        _fill,
      );
    }
    _stroke.color = you;
    canvas.drawPath(_you, _stroke);
    _stroke.color = agent;
    canvas.drawPath(_agent, _stroke);
    canvas.restore();
  }

  @override
  bool shouldRepaint(ClaxSynapsePainter old) =>
      old.model != model ||
      old.you != you ||
      old.agent != agent ||
      old.dot != dot;
}
