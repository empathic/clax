import 'package:flutter/gestures.dart';

import '../daemon/clock.dart';

/// Recognizes a two-finger horizontal swipe from raw pointer events, fed by
/// a `Listener` that stays out of the gesture arena (so the WebView under
/// it still gets every touch).
///
/// The gesture starts when a second finger goes down while one is down;
/// it swipes once both fingers have moved more than [minDx] logical pixels
/// the same way, each with `|dy| < |dx| / 2`, within [window] of that
/// start. A third finger, a cancelled pointer, or the window passing ends
/// the gesture until every finger is lifted.
class TwoFingerSwipe {
  TwoFingerSwipe({
    required this._left,
    required this._right,
    this._clock = const SystemClock(),
  });

  final void Function() _left;
  final void Function() _right;
  final Clock _clock;

  static const minDx = 80.0;
  static const window = Duration(milliseconds: 600);

  /// Each finger down: its position now, and at the gesture's start.
  final _now = <int, Offset>{};
  final _from = <int, Offset>{};
  DateTime? _start;

  /// Whether this touch sequence can no longer swipe (it did, or failed).
  var _done = false;

  void onPointerDown(PointerDownEvent e) {
    _now[e.pointer] = e.position;
    if (_now.length == 2 && !_done) {
      _from
        ..clear()
        ..addAll(_now);
      _start = _clock.now();
    } else if (_now.length > 2) {
      _done = true;
    }
  }

  void onPointerMove(PointerMoveEvent e) {
    if (!_now.containsKey(e.pointer)) return;
    _now[e.pointer] = e.position;
    final start = _start;
    if (_done || start == null || _from.length != 2) return;
    if (_clock.now().difference(start) > window) {
      _done = true;
      return;
    }
    var sign = 0;
    for (final p in _from.keys) {
      final at = _now[p];
      if (at == null) return;
      final d = at - _from[p]!;
      if (d.dx.abs() <= minDx || d.dy.abs() >= d.dx.abs() / 2) return;
      final s = d.dx.sign.toInt();
      if (sign != 0 && s != sign) return;
      sign = s;
    }
    _done = true;
    sign < 0 ? _left() : _right();
  }

  void onPointerUp(PointerUpEvent e) => _lift(e.pointer, cancelled: false);

  void onPointerCancel(PointerCancelEvent e) =>
      _lift(e.pointer, cancelled: true);

  void _lift(int pointer, {required bool cancelled}) {
    if (_now.remove(pointer) == null) return;
    if (_from.containsKey(pointer)) {
      // One of the gesture's fingers left before it swiped.
      _done = true;
    }
    if (cancelled) _done = true;
    if (_now.isEmpty) {
      _from.clear();
      _start = null;
      _done = false;
    }
  }
}
