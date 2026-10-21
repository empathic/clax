import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'mark.dart';

/// The Clax mark as a synapse, for a wait with something to show for it
/// (an app's update): the arcs draw apart and the dot fires across the gap
/// between them like a signal between two neurons, the person's arc (left)
/// and Clax's (right) taking turns.
///
/// A firing charges its arc (a glow and a swell), sends the dot across
/// with a wind-up, a quick ragged path, a stretch with its speed and a
/// trail in the firing arc's colour, with three to five smaller ions loosed
/// from the arc's face beside it; the arc it lands on nods and glows. The
/// rests between firings vary, from a quick return to a long pause, drawn
/// from [seed].
///
/// When [finishing] turns true the dot flies to Clax's arc, which takes it
/// with a nod and a sparkle while the arcs close up, and a new dot grows in
/// the middle: the resting [ClaxMark], [finishLength] later, when
/// [onFinished] is called.
///
/// Under reduced motion (`MediaQuery.disableAnimationsOf`) or a quiet
/// [ClaxQuiet] it is the resting [ClaxMark], and a finish ends at once.
class ClaxSynapse extends StatefulWidget {
  const ClaxSynapse({
    super.key,
    this.size = 112,
    this.finishing = false,
    this.onFinished,
    this.seed,
  });

  /// The width it draws in, the arcs drawn apart; the closed mark is
  /// [size] × 22/28 wide, and the height [size] × 16/28.
  final double size;

  /// Whether to play the finish: once, from when it turns true.
  final bool finishing;

  /// Called once a finish has played (at once without motion).
  final VoidCallback? onFinished;

  /// Seeds the rhythm; null for a fresh one.
  final int? seed;

  /// How long a finish plays.
  static const finishLength = Duration(milliseconds: 420);

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
    final size = Size(
      widget.size,
      widget.size * ClaxMarkGeometry.height / ClaxSynapseModel.width,
    );
    if (_still) {
      return SizedBox.fromSize(
        size: size,
        child: Center(
          child: ClaxMark(
            size: widget.size * ClaxMarkGeometry.width / ClaxSynapseModel.width,
          ),
        ),
      );
    }
    final p = claxPalette(context);
    return CustomPaint(
      size: size,
      painter: ClaxSynapsePainter(
        model: _model,
        you: p.you,
        agent: p.agent,
        dot: p.fg,
      ),
    );
  }
}

/// One firing: the dot crossing the gap from one arc to the other at
/// [start] ms for [travel] ms, with its path's wobble and its [ions].
class _Firing {
  _Firing(this.start, this.toAgent, this.travel, math.Random r)
    : wobble = (r.nextBool() ? 1 : -1) * (0.55 + 0.45 * r.nextDouble()),
      wobbleFreq = 0.8 + 0.6 * r.nextDouble(),
      wobblePhase = r.nextDouble() * 2 * math.pi,
      jitter = 0.18 + 0.14 * r.nextDouble(),
      jitterFreq = 3.5 + 2.5 * r.nextDouble(),
      jitterPhase = r.nextDouble() * 2 * math.pi,
      ions = Float64List(_ionFields * _maxIons),
      ionCount = 3 + r.nextInt(_maxIons - 2) {
    for (var i = 0; i < ionCount; i++) {
      final o = i * _ionFields;
      final from = (r.nextDouble() * 2 - 1) * 0.95;
      ions[o] = 20 + r.nextDouble() * 140; // delay, ms
      ions[o + 1] = 0.85 + r.nextDouble() * 0.5; // travel × this
      ions[o + 2] = from; // angle on the firing arc's face
      ions[o + 3] = from * 0.3 + (r.nextDouble() * 2 - 1) * 0.25; // landing
      ions[o + 4] = 0.42 + r.nextDouble() * 0.36; // radius
      ions[o + 5] = (r.nextDouble() * 2 - 1) * 0.9; // wobble
      ions[o + 6] = r.nextDouble() * 2 * math.pi; // wobble phase
    }
  }

  static const _ionFields = 7, _maxIons = 5;

  final double start, travel;
  final bool toAgent;
  final double wobble, wobbleFreq, wobblePhase;
  final double jitter, jitterFreq, jitterPhase;
  final Float64List ions;
  final int ionCount;

  double get end => start + travel;

  /// The dot's sideways offset at progress [p] (0…1): one slow swing and a
  /// quick tremor, both pinned at the ends.
  double offsetAt(double p) {
    final env = math.sin(math.pi * p.clamp(0.0, 1.0));
    return env *
        (wobble * math.sin(2 * math.pi * wobbleFreq * p + wobblePhase) +
            jitter * math.sin(2 * math.pi * jitterFreq * p + jitterPhase));
  }
}

/// The synapse's motion over time, in [ClaxMarkGeometry]'s units in a
/// [width] by [ClaxMarkGeometry.height] box: where the arcs and the dot
/// are at the time [seek] last set, and what glows.
///
/// The rhythm is drawn from a seeded random source, firing by firing as
/// time reaches them; [finishAt] starts the finish.
class ClaxSynapseModel extends ChangeNotifier {
  ClaxSynapseModel({int? seed}) : _random = math.Random(seed);

  /// The box's width: the mark's 22 and room for the arcs to part.
  static const double width = 28;

  /// Where the dot rests in the middle: the mark's dot, centred in the box.
  static const double centreX = width / 2;

  /// How far each arc moves out as the synapse opens.
  static const double part = 3;

  /// How long the opening takes, and when the first firing leaves.
  static const double openMs = 320, firstFire = 420;

  /// The finish's beats: the dot lands at [landMs] and is taken by
  /// [absorbMs]; the arcs close from [closeMs], and the new dot is grown
  /// by [finishMs].
  static const double landMs = 180, absorbMs = 250, closeMs = 200;
  static const double finishMs = 420;

  /// How far the dot rests from an arc's face.
  static const double _margin = 0.35;

  /// The wind-up, dart and settle of a firing.
  static const Curve _fire = Cubic(0.55, -0.22, 0.25, 1);

  /// easeOutBack: a growth that overshoots a little and settles.
  static const Curve _overshoot = Cubic(0.34, 1.56, 0.64, 1);

  final math.Random _random;
  final List<_Firing> _firings = [];

  /// Milliseconds since it began.
  double get time => _t;
  double _t = 0;

  /// When the finish began, or null before it.
  double? get finishStart => _finishStart;
  double? _finishStart;
  double _finishFromX = 0, _finishFromY = 0, _finishFromOpen = 0;
  bool _finishHop = false;

  /// Whether the finish has played through.
  bool get finished => _finishStart != null && _t >= _finishStart! + finishMs;

  // The pose at [time], set by [seek].

  /// How far apart the arcs are, 0 (the mark) to 1 (the open synapse).
  double open = 0;

  /// Each arc's scale about its own centre, and its glow (0…1).
  double youScale = 1, agentScale = 1, youGlow = 0, agentGlow = 0;

  /// The dot's centre, its scale, its stretch along its path's [dotAngle].
  double dotX = centreX, dotY = ClaxMarkGeometry.dot.dy;
  double dotScale = 1, dotStretch = 1, dotAngle = 0;

  /// The flying dot's glow and trail (0…1), and whether they are in the
  /// person's colour (fired from the person's arc) or Clax's.
  double flight = 0;
  bool flightFromYou = true;

  /// The trail: [trailCount] earlier centres, newest first, as x, y pairs.
  final Float64List trail = Float64List(2 * _trailLength);
  int trailCount = 0;
  static const _trailLength = 6;

  /// The ions in flight: [ionCount] of x, y, radius, opacity, and 1 when
  /// in the person's colour, else 0.
  final Float64List ions = Float64List(5 * 2 * _Firing._maxIons);
  int ionCount = 0;

  /// The sparkle's progress, 0…1, while it plays; null otherwise.
  double? sparkle;

  /// The x the dot rests at beside the person's arc (`toAgent` false) or
  /// Clax's, with the arcs [o] apart.
  static double restX(bool atAgent, double o, {double r = 1.7}) {
    final face = ClaxMarkGeometry.stroke / 2 + 5.5;
    final inset = (width - ClaxMarkGeometry.width) / 2;
    return atAgent
        ? inset + 20.5 + part * o - face - r - _margin
        : inset + 1.5 - part * o + face + r + _margin;
  }

  /// The point on an arc's face at [angle] radians from its apex, the arcs
  /// [o] apart, into [out] at [at].
  static void _facePoint(
    bool agent,
    double angle,
    double o,
    Float64List out,
    int at,
  ) {
    final inset = (width - ClaxMarkGeometry.width) / 2;
    const face = 5.5 + 1.3 + 0.2;
    final cx = agent ? inset + 20.5 + part * o : inset + 1.5 - part * o;
    out[at] = cx + (agent ? -1 : 1) * face * math.cos(angle);
    out[at + 1] = ClaxMarkGeometry.dot.dy + face * math.sin(angle);
  }

  static final Float64List _scratch = Float64List(4);

  /// The firing under way or last ended at [t], drawing new ones as
  /// time reaches them; null before the first.
  _Firing? _firingAt(double t) {
    if (_firings.isEmpty) {
      _firings.add(_Firing(firstFire, true, _travel(), _random));
    }
    // Always one firing still to come: the one whose charge builds next.
    while (_firings.last.start <= t) {
      final last = _firings.last;
      _firings.add(
        _Firing(last.end + _rest(), !last.toAgent, _travel(), _random),
      );
    }
    // Keep what the trail, the glows and the nods still read.
    while (_firings.length > 3 && _firings[1].end + 600 < t) {
      _firings.removeAt(0);
    }
    for (var i = _firings.length - 1; i >= 0; i--) {
      if (_firings[i].start <= t) return _firings[i];
    }
    return null;
  }

  double _travel() => 340 + _random.nextDouble() * 200;

  /// A rest after a landing: mostly a beat, sometimes a quick return,
  /// sometimes a long pause.
  double _rest() {
    final roll = _random.nextDouble();
    if (roll < 0.22) return 110 + _random.nextDouble() * 110;
    if (roll < 0.88) return 340 + _random.nextDouble() * 400;
    return 760 + _random.nextDouble() * 300;
  }

  /// [curve] over [from]…[to] at [t]: 0 before, 1 after.
  static double _span(double t, double from, double to, [Curve? curve]) {
    final x = ((t - from) / (to - from)).clamp(0.0, 1.0);
    return curve == null ? x : curve.transform(x);
  }

  /// Starts the finish at [ms], from wherever the dot is then.
  void finishAt(double ms) {
    seek(ms);
    _finishStart = ms;
    _finishFromX = dotX;
    _finishFromY = dotY;
    _finishFromOpen = open;
    _finishHop = (dotX - restX(true, open)).abs() < 1;
  }

  /// Sets the pose to [ms] after it began.
  void seek(double ms) {
    _t = ms;
    final fs = _finishStart;
    if (fs == null) {
      _run(ms);
    } else {
      _finish(ms - fs);
    }
    notifyListeners();
  }

  void _run(double t) {
    open = _span(t, 0, openMs, ClaxMotion.decelerate);
    youScale = agentScale = 1;
    youGlow = agentGlow = 0;
    dotScale = 1;
    dotStretch = 1;
    dotAngle = 0;
    flight = 0;
    trailCount = 0;
    ionCount = 0;
    sparkle = null;
    final f = _firingAt(t);
    if (f == null) {
      // Opening: the dot drifts to the person's arc to fire first.
      final p = _span(t, 80, firstFire, ClaxMotion.standard);
      dotX = _mix(centreX, restX(false, open), p);
      dotY = ClaxMarkGeometry.dot.dy;
      _charge(_firings.first, t);
      return;
    }
    final p = (t - f.start) / f.travel;
    if (p < 1) {
      _fly(f, p, open);
      flight = math.sin(math.pi * p);
      flightFromYou = f.toAgent;
      _trail(f, p, open);
      _ions(f, t, open, 1);
    } else {
      dotX = restX(f.toAgent, open);
      dotY = ClaxMarkGeometry.dot.dy;
      // A slight breath while it waits.
      dotScale = 1 + 0.06 * math.sin(2 * math.pi * (t - f.end) / 900);
      _ions(f, t, open, 1);
    }
    // The landings' nods and glows, the next firing's charge.
    for (final g in _firings) {
      if (g.end > t) continue;
      final since = t - g.end;
      final nod = 1 - _span(since, 0, 200, ClaxMotion.decelerate);
      final glow = 1 - _span(since, 0, 520, ClaxMotion.decelerate);
      if (g.toAgent) {
        agentScale = math.max(agentScale, 1 + 0.09 * nod);
        agentGlow = math.max(agentGlow, glow);
      } else {
        youScale = math.max(youScale, 1 + 0.09 * nod);
        youGlow = math.max(youGlow, glow);
      }
    }
    final next = _firings.last;
    if (next.start > t) _charge(next, t);
    if (p < 1) _charge(f, t);
  }

  /// The firing arc's charge before [f] leaves and its release after.
  void _charge(_Firing f, double t) {
    final charge =
        _span(t, f.start - 180, f.start, ClaxMotion.accelerate) -
        _span(t, f.start, f.start + 140, ClaxMotion.decelerate);
    if (charge <= 0) return;
    if (f.toAgent) {
      youGlow = math.max(youGlow, 0.7 * charge);
      youScale = math.max(youScale, 1 + 0.04 * charge);
    } else {
      agentGlow = math.max(agentGlow, 0.7 * charge);
      agentScale = math.max(agentScale, 1 + 0.04 * charge);
    }
  }

  /// The dot along [f] at progress [p], with its stretch and heading.
  void _fly(_Firing f, double p, double o) {
    final (x, y) = _flightAt(f, p, o);
    dotX = x;
    dotY = y;
    final (ax, ay) = _flightAt(f, (p - 0.03).clamp(0.0, 1.0), o);
    final (bx, by) = _flightAt(f, (p + 0.03).clamp(0.0, 1.0), o);
    final dx = bx - ax, dy = by - ay;
    final speed = math.sqrt(dx * dx + dy * dy) / 0.06;
    dotAngle = math.atan2(dy, dx);
    dotStretch = 1 + (speed / 40).clamp(0.0, 0.45);
    dotScale = 1 - 0.1 * math.sin(math.pi * p);
  }

  (double, double) _flightAt(_Firing f, double p, double o) {
    final from = restX(!f.toAgent, o), to = restX(f.toAgent, o);
    return (
      _mix(from, to, _fire.transform(p)),
      ClaxMarkGeometry.dot.dy + f.offsetAt(p),
    );
  }

  void _trail(_Firing f, double p, double o) {
    var n = 0;
    for (var k = 1; k <= _trailLength; k++) {
      final q = p - k * 0.035;
      if (q <= 0) break;
      final (x, y) = _flightAt(f, q, o);
      trail[2 * n] = x;
      trail[2 * n + 1] = y;
      n++;
    }
    trailCount = n;
  }

  /// [f]'s ions in flight at [t], at [fade] of their opacity.
  void _ions(_Firing f, double t, double o, double fade) {
    var n = ionCount;
    for (var i = 0; i < f.ionCount; i++) {
      final at = i * _Firing._ionFields;
      final start = f.start + f.ions[at];
      final q = (t - start) / (f.travel * f.ions[at + 1]);
      if (q <= 0 || q >= 1) continue;
      _facePoint(!f.toAgent, f.ions[at + 2], o, _scratch, 0);
      _facePoint(f.toAgent, f.ions[at + 3], o, _scratch, 2);
      final e = ClaxMotion.standard.transform(q);
      final wobble =
          f.ions[at + 5] *
          math.sin(math.pi * q) *
          math.sin(3 * math.pi * q + f.ions[at + 6]);
      final out = n * 5;
      ions[out] = _mix(_scratch[0], _scratch[2], e);
      ions[out + 1] = _mix(_scratch[1], _scratch[3], e) + wobble;
      ions[out + 2] = f.ions[at + 4];
      ions[out + 3] = fade * _span(q, 0, 0.15) * (1 - _span(q, 0.8, 1));
      ions[out + 4] = f.toAgent ? 1 : 0;
      n++;
      if (n * 5 >= ions.length) break;
    }
    ionCount = n;
  }

  void _finish(double e) {
    final fade = 1 - _span(e, 0, 200);
    // The firings under way fade out where they are.
    _run(_finishStart!);
    youGlow *= fade;
    agentGlow *= fade;
    youScale = 1 + (youScale - 1) * fade;
    agentScale = 1 + (agentScale - 1) * fade;
    final f = _firingAt(_finishStart!);
    ionCount = 0;
    if (f != null) {
      _ions(f, _finishStart! + e, _finishFromOpen, 1 - _span(e, 0, 140));
    }
    trailCount = 0;
    flight = 0;
    dotStretch = 1;
    dotAngle = 0;

    open =
        _finishFromOpen *
        (1 - _span(e, closeMs, finishMs - 20, ClaxMotion.standard));
    // To Clax's arc, then taken, then a new dot in the middle as the arcs
    // close up.
    if (e < landMs) {
      final p = _span(e, 0, landMs, ClaxMotion.decelerate);
      dotX = _mix(_finishFromX, restX(true, open), p);
      dotY =
          _mix(_finishFromY, ClaxMarkGeometry.dot.dy, p) -
          (_finishHop ? 1.6 * math.sin(math.pi * _span(e, 0, landMs)) : 0);
      dotScale = 1;
      flight = math.sin(math.pi * _span(e, 0, landMs)) * 0.8;
      flightFromYou = true;
    } else if (e < absorbMs) {
      dotX = restX(true, open) + 1.2 * _span(e, landMs, absorbMs);
      dotY = ClaxMarkGeometry.dot.dy;
      dotScale = 1 - _span(e, landMs, absorbMs, ClaxMotion.accelerate);
    } else {
      dotX = centreX;
      dotY = ClaxMarkGeometry.dot.dy;
      dotScale = e >= finishMs
          ? 1
          : _overshoot.transform(_span(e, absorbMs, finishMs));
    }
    final nod =
        _span(e, landMs, landMs + 70, ClaxMotion.decelerate) -
        _span(e, landMs + 70, finishMs, ClaxMotion.standard);
    agentScale = math.max(agentScale, 1 + 0.12 * nod);
    final glow =
        _span(e, landMs - 40, landMs, ClaxMotion.accelerate) -
        _span(e, landMs, finishMs, ClaxMotion.decelerate);
    agentGlow = math.max(agentGlow, glow);
    sparkle = e >= landMs && e < finishMs
        ? _span(e, landMs, finishMs - 20)
        : null;
    if (e >= finishMs) {
      open = 0;
      youScale = agentScale = 1;
      youGlow = agentGlow = 0;
      ionCount = 0;
      sparkle = null;
    }
  }
}

double _mix(double a, double b, double t) => a + (b - a) * t;

/// Paints a [ClaxSynapseModel]'s pose, repainting as it moves: the arcs in
/// [you] and [agent] with their glows, the dot in [dot], and the trail,
/// ions and sparkle in the arcs' colours.
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
  static final Offset _youCentre = _you.getBounds().center;
  static final Offset _agentCentre = _agent.getBounds().center;

  final Paint _stroke = Paint()
    ..style = PaintingStyle.stroke
    ..strokeWidth = ClaxMarkGeometry.stroke
    ..strokeCap = ClaxMarkGeometry.cap
    ..isAntiAlias = true;
  final Paint _glow = Paint()
    ..style = PaintingStyle.stroke
    ..strokeWidth = ClaxMarkGeometry.stroke * 2.6
    ..strokeCap = ClaxMarkGeometry.cap
    ..isAntiAlias = true;
  final Paint _fill = Paint()..isAntiAlias = true;
  final Paint _soft = Paint()..isAntiAlias = true;
  final Paint _ray = Paint()
    ..style = PaintingStyle.stroke
    ..strokeCap = StrokeCap.round
    ..isAntiAlias = true;
  double? _blurFor;

  void _arc(
    Canvas canvas,
    Path path,
    Offset centre,
    double dx,
    double k,
    Color c,
    double glow,
  ) {
    canvas.save();
    canvas.translate(dx + centre.dx, centre.dy);
    canvas.scale(k);
    canvas.translate(-centre.dx, -centre.dy);
    if (glow > 0.01) {
      _glow.color = c.withValues(alpha: 0.42 * glow);
      canvas.drawPath(path, _glow);
    }
    _stroke.color = c;
    canvas.drawPath(path, _stroke);
    canvas.restore();
  }

  @override
  void paint(Canvas canvas, Size size) {
    final m = model;
    final s = size.width / ClaxSynapseModel.width;
    if (_blurFor != s) {
      _blurFor = s;
      // Blur radii in device pixels: a glow about one unit soft.
      _glow.maskFilter = MaskFilter.blur(BlurStyle.normal, 1.1 * s);
      _soft.maskFilter = MaskFilter.blur(BlurStyle.normal, 1.3 * s);
    }
    canvas.save();
    canvas.scale(s);
    const inset = (ClaxSynapseModel.width - ClaxMarkGeometry.width) / 2;
    final shift = ClaxSynapseModel.part * m.open;
    _arc(canvas, _you, _youCentre, inset - shift, m.youScale, you, m.youGlow);
    _arc(
      canvas,
      _agent,
      _agentCentre,
      inset + shift,
      m.agentScale,
      agent,
      m.agentGlow,
    );

    final tint = m.flightFromYou ? you : agent;
    // The trail, newest first, fading and shrinking.
    for (var k = 0; k < m.trailCount; k++) {
      final f = 1 - (k + 1) / (m.trailCount + 1.5);
      _fill.color = tint.withValues(alpha: 0.42 * f * m.flight);
      canvas.drawCircle(
        Offset(m.trail[2 * k], m.trail[2 * k + 1]),
        ClaxMarkGeometry.dotRadius * (0.35 + 0.55 * f),
        _fill,
      );
    }
    // The ions.
    for (var i = 0; i < m.ionCount; i++) {
      final o = i * 5;
      final c = m.ions[o + 4] == 1 ? you : agent;
      final at = Offset(m.ions[o], m.ions[o + 1]);
      _soft.color = c.withValues(alpha: 0.35 * m.ions[o + 3]);
      canvas.drawCircle(at, m.ions[o + 2] * 2.2, _soft);
      _fill.color = c.withValues(alpha: m.ions[o + 3]);
      canvas.drawCircle(at, m.ions[o + 2], _fill);
    }
    // The dot, with its glow in flight, stretched along its path.
    final centre = Offset(m.dotX, m.dotY);
    if (m.flight > 0.01) {
      _soft.color = tint.withValues(alpha: 0.4 * m.flight);
      canvas.drawCircle(centre, ClaxMarkGeometry.dotRadius * 2.3, _soft);
    }
    final r = math.max(0.0, ClaxMarkGeometry.dotRadius * m.dotScale);
    if (r > 0) {
      _fill.color = dot;
      canvas.save();
      canvas.translate(centre.dx, centre.dy);
      canvas.rotate(m.dotAngle);
      final squash = 1 / math.sqrt(m.dotStretch);
      canvas.drawOval(
        Rect.fromCenter(
          center: Offset.zero,
          width: 2 * r * m.dotStretch,
          height: 2 * r * squash,
        ),
        _fill,
      );
      canvas.restore();
    }
    // The sparkle where the dot landed: a fan of rays and specks from
    // Clax's arc into the gap, and a glint at each of its tips.
    final sp = m.sparkle;
    if (sp != null) {
      final face =
          ClaxSynapseModel.restX(true, m.open) + ClaxMarkGeometry.dotRadius;
      final from = Offset(face, ClaxMarkGeometry.dot.dy);
      final e = ClaxMotion.decelerate.transform(sp);
      final fade = 1 - sp * sp;
      for (var k = 0; k < 7; k++) {
        final a = math.pi + (k - 3) * 0.4;
        final inner = 1.3 + 4.2 * e;
        final outer = inner + 1.7 * (1 - e) + 0.3;
        final dir = Offset(math.cos(a), math.sin(a));
        _ray
          ..color = (k.isEven ? agent : you).withValues(alpha: fade)
          ..strokeWidth = 0.8 * (1 - 0.5 * e);
        canvas.drawLine(from + dir * inner, from + dir * outer, _ray);
      }
      _fill.color = dot.withValues(alpha: fade);
      for (var k = 0; k < 4; k++) {
        final a = math.pi + (k - 1.5) * 0.8;
        canvas.drawCircle(
          from + Offset(math.cos(a), math.sin(a)) * (1.8 + 4.6 * e),
          0.42 * (1 - 0.4 * e),
          _fill,
        );
      }
      const inset = (ClaxSynapseModel.width - ClaxMarkGeometry.width) / 2;
      final tipX = inset + 20.5 + ClaxSynapseModel.part * m.open;
      for (final (y, up) in [(2.5, -1.0), (13.5, 1.0)]) {
        for (final side in [-1.0, 1.0]) {
          final a = up * math.pi / 2 + side * 0.55;
          final dir = Offset(math.cos(a), math.sin(a));
          final tip = Offset(tipX, y);
          _ray
            ..color = agent.withValues(alpha: fade)
            ..strokeWidth = 0.7 * (1 - 0.5 * e);
          canvas.drawLine(
            tip + dir * (1.8 + 2.0 * e),
            tip + dir * (1.8 + 2.0 * e + 1.1 * (1 - e) + 0.2),
            _ray,
          );
        }
      }
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(ClaxSynapsePainter old) =>
      old.model != model ||
      old.you != you ||
      old.agent != agent ||
      old.dot != dot;
}
