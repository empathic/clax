import 'dart:math' as math;
import 'dart:typed_data';

import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import '../theme.dart';
import '../tokens.g.dart';

/// What the mark is doing. The dot is the conversation between the person
/// (the orange arc at the left) and Clax (the green arc at the right).
enum ClaxMarkState {
  /// Still: the splash, empty states, the top bar.
  resting,

  /// The dot breathes toward Clax and back, every `mark-breath`.
  working,

  /// Once, in 300 ms: the dot slides into Clax's arc, which takes it with a
  /// small nod, and a new dot grows in its place.
  sent,

  /// Once, in 300 ms: Clax's arc nods (scale 1.08 and back) and the dot
  /// steps toward the person and back.
  landed,

  /// The dot breathes toward the person and back, every `mark-breath`.
  asking,

  /// Once, about 2.3 s: the dot rallies between the arcs, each arc nodding
  /// as it returns the dot, ten hits in all, quickening, then it comes to
  /// rest in the middle.
  rally,
}

/// Turns every [ClaxMark] under it still while [quiet] (comment mode), as
/// reduced motion does.
class ClaxQuiet extends InheritedWidget {
  const ClaxQuiet({super.key, required this.quiet, required super.child});

  final bool quiet;

  /// Whether the nearest [ClaxQuiet] above [c] is quiet; false with none.
  static bool of(BuildContext c) =>
      c.dependOnInheritedWidgetOfExactType<ClaxQuiet>()?.quiet ?? false;

  @override
  bool updateShouldNotify(ClaxQuiet old) => quiet != old.quiet;
}

/// The Clax mark, `) · (`, drawn from [ClaxMarkGeometry] in [state].
///
/// Under reduced motion (`MediaQuery.disableAnimationsOf`) and under a quiet
/// [ClaxQuiet] every state draws still: [ClaxMarkState.working] leaning
/// toward Clax and [ClaxMarkState.asking] toward the person as far as their
/// breath goes, the others at rest. A change of state glides the dot from
/// where it was.
class ClaxMark extends StatefulWidget {
  const ClaxMark({
    super.key,
    this.size = 24,
    this.state = ClaxMarkState.resting,
    this.monochrome,
    this.inverse = false,
    this.semanticsLabel,
  });

  /// The mark's width; its height is [size] × 16/22.
  final double size;
  final ClaxMarkState state;

  /// One colour for all three shapes (icons, the pill on `fg`); null draws
  /// the arcs in `you` and `agent` and the dot in `fg`.
  final Color? monochrome;

  /// Drawn on `fg` (the Clax pill, a toast): the arcs in the other
  /// theme's `you` and `agent`, which hold their contrast on it, and the
  /// dot in `bg`.
  final bool inverse;

  /// What a screen reader says; null says [defaultLabel] of [state].
  final String? semanticsLabel;

  /// The words for [state]: none at rest.
  static String? defaultLabel(ClaxMarkState state) => switch (state) {
    ClaxMarkState.resting => null,
    ClaxMarkState.working => 'Clax is working',
    ClaxMarkState.sent => 'Sent to Clax',
    ClaxMarkState.landed || ClaxMarkState.rally => 'New version ready',
    ClaxMarkState.asking => 'Clax has a question',
  };

  /// When the dot meets an arc in [ClaxMarkState.rally], from its start:
  /// ten hits, quickening.
  static List<Duration> get rallyHits => [
    for (final ms in _rallyHits) Duration(microseconds: (ms * 1000).round()),
  ];

  @override
  State<ClaxMark> createState() => _ClaxMarkState();
}

/// The palette of the other brightness at the same contrast as [p].
ClaxPalette _inverseOf(ClaxPalette p) {
  if (identical(p, ClaxTokens.light)) return ClaxTokens.dark;
  if (identical(p, ClaxTokens.dark)) return ClaxTokens.light;
  if (identical(p, ClaxTokens.lightMore)) return ClaxTokens.darkMore;
  if (identical(p, ClaxTokens.darkMore)) return ClaxTokens.lightMore;
  return p.bg.computeLuminance() > 0.5 ? ClaxTokens.dark : ClaxTokens.light;
}

/// Where the mark's moving parts are: the dot's shift (−1 touches the
/// person's arc, +1 Clax's) and size, and each arc's scale about its own
/// centre.
@immutable
class ClaxMarkPose {
  const ClaxMarkPose({
    this.dotShift = 0,
    this.dotScale = 1,
    this.youScale = 1,
    this.agentScale = 1,
  });

  final double dotShift, dotScale, youScale, agentScale;

  static const rest = ClaxMarkPose();

  static ClaxMarkPose lerp(ClaxMarkPose a, ClaxMarkPose b, double t) =>
      ClaxMarkPose(
        dotShift: _mix(a.dotShift, b.dotShift, t),
        dotScale: _mix(a.dotScale, b.dotScale, t),
        youScale: _mix(a.youScale, b.youScale, t),
        agentScale: _mix(a.agentScale, b.agentScale, t),
      );
}

double _mix(double a, double b, double t) => a + (b - a) * t;

/// The breath's reach: how far the dot drifts, and how much it swells.
const double _breathShift = 0.35, _breathSwell = 0.12;

/// How long a change of state glides the dot from where it was.
const Duration _glide = ClaxMotion.base;

/// easeOutBack: a growth that overshoots a little and settles.
const Curve _overshoot = Cubic(0.34, 1.56, 0.64, 1);

/// [curve] over the span [from]…[to] ms at [t] ms: 0 before, 1 after.
double _span(double t, double from, double to, Curve curve) =>
    curve.transform(((t - from) / (to - from)).clamp(0.0, 1.0));

/// The rally's legs: 120 ms to the first hit, nine crossings quickening from
/// 240 ms to 180 ms, and 240 ms back to the middle.
final List<double> _rallyHits = () {
  final hits = <double>[120];
  for (var i = 0; i < 9; i++) {
    hits.add(hits.last + 240 - i * 7.5);
  }
  return hits;
}();
const double _rallySettle = 240;
const double _rallyNod = 140;

double get _rallyLength => _rallyHits.last + _rallySettle;

/// How long [state] runs once; null for the breaths, which loop.
double? _length(ClaxMarkState state) => switch (state) {
  ClaxMarkState.resting => 0,
  ClaxMarkState.sent || ClaxMarkState.landed => 300,
  ClaxMarkState.rally => _rallyLength,
  ClaxMarkState.working || ClaxMarkState.asking => null,
};

/// [state]'s pose [t] ms after it began.
ClaxMarkPose _poseAt(ClaxMarkState state, double t) {
  switch (state) {
    case ClaxMarkState.resting:
      return ClaxMarkPose.rest;
    case ClaxMarkState.working:
    case ClaxMarkState.asking:
      final ms = ClaxMotion.markBreath.inMicroseconds / 1000;
      final breath = (1 - math.cos(2 * math.pi * t / ms)) / 2;
      final toward = state == ClaxMarkState.working ? 1 : -1;
      return ClaxMarkPose(
        dotShift: toward * _breathShift * breath,
        dotScale: 1 + _breathSwell * breath,
      );
    case ClaxMarkState.sent:
      if (t >= 300) return ClaxMarkPose.rest;
      final nod =
          _span(t, 160, 220, ClaxMotion.decelerate) -
          _span(t, 220, 300, ClaxMotion.standard);
      if (t <= 180) {
        return ClaxMarkPose(
          dotShift:
              0.9 * _span(t, 0, 120, ClaxMotion.decelerate) +
              0.1 * _span(t, 120, 180, ClaxMotion.accelerate),
          dotScale: 1 - _span(t, 120, 180, ClaxMotion.accelerate),
          agentScale: 1 + 0.05 * nod,
        );
      }
      return ClaxMarkPose(
        dotScale: _overshoot.transform((t - 180) / 120),
        agentScale: 1 + 0.05 * nod,
      );
    case ClaxMarkState.landed:
      if (t >= 300) return ClaxMarkPose.rest;
      final nod =
          _span(t, 0, 120, ClaxMotion.decelerate) -
          _span(t, 120, 300, ClaxMotion.standard);
      final step =
          _span(t, 90, 180, ClaxMotion.decelerate) -
          _span(t, 180, 300, ClaxMotion.standard);
      return ClaxMarkPose(agentScale: 1 + 0.08 * nod, dotShift: -0.2 * step);
    case ClaxMarkState.rally:
      if (t >= _rallyLength) return ClaxMarkPose.rest;
      final hits = _rallyHits;
      // The dot: out to the first hit, linear crossings, back to the middle.
      double shift;
      if (t <= hits.first) {
        shift = t / hits.first;
      } else if (t >= hits.last) {
        final side = hits.length.isOdd ? 1.0 : -1.0;
        shift = side * (1 - _span(t, hits.last, _rallyLength, ClaxMotion.decelerate));
      } else {
        var i = 0;
        while (hits[i + 1] < t) {
          i++;
        }
        final from = i.isEven ? 1.0 : -1.0;
        shift = _mix(from, -from, (t - hits[i]) / (hits[i + 1] - hits[i]));
      }
      // Each arc nods as it returns the dot.
      var you = 1.0, agent = 1.0;
      for (final (i, hit) in hits.indexed) {
        if (t < hit || t > hit + _rallyNod) continue;
        final nod = 1 + 0.08 * (1 - ClaxMotion.decelerate.transform((t - hit) / _rallyNod));
        if (i.isEven) {
          agent = nod;
        } else {
          you = nod;
        }
      }
      return ClaxMarkPose(dotShift: shift, youScale: you, agentScale: agent);
  }
}

/// [state]'s pose when nothing may move: a breath held at its widest lean,
/// everything else at rest.
ClaxMarkPose _stillPose(ClaxMarkState state) => switch (state) {
  ClaxMarkState.working => const ClaxMarkPose(dotShift: _breathShift),
  ClaxMarkState.asking => const ClaxMarkPose(dotShift: -_breathShift),
  _ => ClaxMarkPose.rest,
};

class _ClaxMarkState extends State<ClaxMark>
    with SingleTickerProviderStateMixin {
  late final Ticker _ticker = createTicker(_tick);

  /// The frame time [ClaxMark.state] began at, or null until the first tick.
  Duration? _start;

  /// Milliseconds since [ClaxMark.state] began.
  double _t = 0;

  /// The pose a change of state glides from, while it glides.
  ClaxMarkPose? _from;
  bool _still = false;

  /// A one-off state that ran or was skipped draws at rest.
  bool _done = false;

  /// The pose the last build drew; null before the first.
  ClaxMarkPose? _drawn;

  ClaxMarkPose get _pose {
    if (_still) return _stillPose(widget.state);
    final to = _done ? ClaxMarkPose.rest : _poseAt(widget.state, _t);
    final from = _from;
    if (from == null) return to;
    final glide = _glide.inMicroseconds / 1000;
    return ClaxMarkPose.lerp(from, to, _span(_t, 0, glide, ClaxMotion.decelerate));
  }

  bool get _moving {
    if (_still) return false;
    final length = _length(widget.state);
    final gliding = _from != null && _t < _glide.inMicroseconds / 1000;
    return gliding || length == null || (!_done && _t < length);
  }

  /// The current frame's time, or null outside a frame.
  static Duration? _frameTime() {
    final b = SchedulerBinding.instance;
    return b.schedulerPhase == SchedulerPhase.idle ? null : b.currentFrameTimeStamp;
  }

  /// Starts [ClaxMark.state] from [from], at this frame.
  void _begin({ClaxMarkPose? from, bool done = false}) {
    _from = from;
    _done = done;
    _t = 0;
    _start = _frameTime();
    _run();
  }

  void _run() {
    if (_moving) {
      if (!_ticker.isActive) _ticker.start();
    } else if (_ticker.isActive) {
      _ticker.stop();
    }
  }

  void _tick(Duration _) {
    final now = _frameTime();
    if (now == null) return;
    final start = _start ??= now;
    setState(() {
      _t = (now - start).inMicroseconds / 1000;
      final glide = _glide.inMicroseconds / 1000;
      if (_from != null && _t >= glide) _from = null;
      final length = _length(widget.state);
      if (length != null && _t >= length) _done = true;
    });
    _run();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final still = MediaQuery.disableAnimationsOf(context) || ClaxQuiet.of(context);
    final first = _drawn == null;
    if (!first && still == _still) return;
    _still = still;
    if (first) {
      _begin();
    } else if (!still) {
      // Waking glides from the still pose; a one-off state is not replayed.
      _begin(from: _stillPose(widget.state), done: _length(widget.state) != null);
    } else {
      _run();
    }
  }

  @override
  void didUpdateWidget(ClaxMark old) {
    super.didUpdateWidget(old);
    if (old.state != widget.state) _begin(from: _still ? null : _drawn);
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final base = claxPalette(context);
    final p = widget.inverse ? _inverseOf(base) : base;
    final mono = widget.monochrome;
    final pose = _drawn = _pose;
    final paint = CustomPaint(
      size: Size(
        widget.size,
        widget.size * ClaxMarkGeometry.height / ClaxMarkGeometry.width,
      ),
      painter: ClaxMarkPainter(
        you: mono ?? p.you,
        agent: mono ?? p.agent,
        dot: mono ?? (widget.inverse ? base.bg : p.fg),
        dotShift: pose.dotShift,
        agentScale: pose.agentScale,
        youScale: pose.youScale,
        dotScale: pose.dotScale,
      ),
    );
    final label = widget.semanticsLabel ?? ClaxMark.defaultLabel(widget.state);
    if (label == null) return paint;
    return Semantics(container: true, image: true, label: label, child: paint);
  }
}

/// Paints [ClaxMarkGeometry] scaled to the canvas's width: the arcs in
/// [you] and [agent], the dot in [dot]. [dotShift] in −1…1 moves the dot
/// toward the person's arc (−) or Clax's (+), touching it at ±1;
/// [dotScale] sizes the dot, and [youScale] and [agentScale] each arc about
/// its own centre.
class ClaxMarkPainter extends CustomPainter {
  ClaxMarkPainter({
    required this.you,
    required this.agent,
    required this.dot,
    required this.dotShift,
    required this.agentScale,
    this.youScale = 1,
    this.dotScale = 1,
  });

  final Color you, agent, dot;
  final double dotShift, agentScale, youScale, dotScale;

  static final Path _you = ClaxMarkGeometry.you();
  static final Path _agent = ClaxMarkGeometry.agent();

  /// How far the dot travels to touch each arc's stroke.
  static final double _youReach =
      ClaxMarkGeometry.dot.dx -
      ClaxMarkGeometry.dotRadius -
      (_you.getBounds().right + ClaxMarkGeometry.stroke / 2);
  static final double _agentReach =
      _agent.getBounds().left -
      ClaxMarkGeometry.stroke / 2 -
      (ClaxMarkGeometry.dot.dx + ClaxMarkGeometry.dotRadius);

  /// [path] scaled by [k] about its centre, then by [s] about the origin.
  static Path _scaled(Path path, double k, double s) {
    final c = path.getBounds().center;
    final a = s * k;
    return path.transform(
      Float64List.fromList([
        a, 0, 0, 0, //
        0, a, 0, 0, //
        0, 0, 1, 0, //
        s * c.dx * (1 - k), s * c.dy * (1 - k), 0, 1,
      ]),
    );
  }

  @override
  void paint(Canvas canvas, Size size) {
    final s = size.width / ClaxMarkGeometry.width;
    Paint stroke(Color c) => Paint()
      ..color = c
      ..style = PaintingStyle.stroke
      ..strokeWidth = ClaxMarkGeometry.stroke * s
      ..strokeCap = ClaxMarkGeometry.cap
      ..isAntiAlias = true;
    canvas.drawPath(_scaled(_you, youScale, s), stroke(you));
    canvas.drawPath(_scaled(_agent, agentScale, s), stroke(agent));
    final reach = dotShift < 0 ? _youReach : _agentReach;
    final centre = ClaxMarkGeometry.dot + Offset(dotShift * reach, 0);
    canvas.drawCircle(
      centre * s,
      math.max(0, ClaxMarkGeometry.dotRadius * dotScale * s),
      Paint()
        ..color = dot
        ..isAntiAlias = true,
    );
  }

  @override
  bool shouldRepaint(ClaxMarkPainter old) =>
      old.you != you ||
      old.agent != agent ||
      old.dot != dot ||
      old.dotShift != dotShift ||
      old.dotScale != dotScale ||
      old.youScale != youScale ||
      old.agentScale != agentScale;
}
