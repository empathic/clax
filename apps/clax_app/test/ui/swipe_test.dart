import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/swipe.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter_test/flutter_test.dart';

class _Clock implements Clock {
  DateTime t = DateTime.utc(2026, 10, 12);
  @override
  DateTime now() => t;
  @override
  Future<void> delay(Duration d) async => t = t.add(d);
  void advance(int ms) => t = t.add(Duration(milliseconds: ms));
}

void main() {
  late _Clock clock;
  late int lefts;
  late int rights;
  late TwoFingerSwipe swipe;

  setUp(() {
    clock = _Clock();
    lefts = 0;
    rights = 0;
    swipe = TwoFingerSwipe(
      left: () => lefts++,
      right: () => rights++,
      clock: clock,
    );
  });

  void down(int p, Offset at) =>
      swipe.onPointerDown(PointerDownEvent(pointer: p, position: at));
  void move(int p, Offset at) =>
      swipe.onPointerMove(PointerMoveEvent(pointer: p, position: at));
  void up(int p, Offset at) =>
      swipe.onPointerUp(PointerUpEvent(pointer: p, position: at));

  /// Two fingers 100 dp apart move by (dx, dy) in [steps] steps, [ms] apart.
  void twoFingers(double dx, double dy, {int steps = 3, int ms = 50}) {
    down(1, const Offset(300, 300));
    down(2, const Offset(300, 400));
    for (var i = 1; i <= steps; i++) {
      clock.advance(ms);
      move(1, Offset(300 + dx * i / steps, 300 + dy * i / steps));
      move(2, Offset(300 + dx * i / steps, 400 + dy * i / steps));
    }
    up(1, Offset(300 + dx, 300 + dy));
    up(2, Offset(300 + dx, 400 + dy));
  }

  test('two pointers moving left 90 dp within 600 ms swipe left once', () {
    twoFingers(-90, 0);
    expect(lefts, 1);
    expect(rights, 0);
  });

  test('moving on after the swipe does not swipe again', () {
    down(1, const Offset(300, 300));
    down(2, const Offset(300, 400));
    for (var i = 1; i <= 6; i++) {
      clock.advance(30);
      move(1, Offset(300 - 50.0 * i, 300));
      move(2, Offset(300 - 50.0 * i, 400));
    }
    expect(lefts, 1);
  });

  test('two pointers moving right swipe right', () {
    twoFingers(90, 10);
    expect(rights, 1);
    expect(lefts, 0);
  });

  test('one pointer does nothing', () {
    down(1, const Offset(300, 300));
    clock.advance(50);
    move(1, const Offset(150, 300));
    up(1, const Offset(150, 300));
    expect(lefts + rights, 0);
  });

  test('a mostly vertical move does nothing', () {
    twoFingers(-90, -60);
    expect(lefts + rights, 0);
  });

  test('too short does nothing', () {
    twoFingers(-70, 0);
    expect(lefts + rights, 0);
  });

  test('too slow does nothing', () {
    twoFingers(-90, 0, steps: 4, ms: 200);
    expect(lefts + rights, 0);
  });

  test('only one of two fingers moving does nothing', () {
    down(1, const Offset(300, 300));
    down(2, const Offset(300, 400));
    clock.advance(50);
    move(1, const Offset(200, 300));
    expect(lefts, 0);
  });

  test('a third finger cancels the gesture', () {
    down(1, const Offset(300, 300));
    down(2, const Offset(300, 400));
    down(3, const Offset(300, 500));
    clock.advance(50);
    move(1, const Offset(200, 300));
    move(2, const Offset(200, 400));
    expect(lefts, 0);
  });

  test('a cancelled pointer ends the gesture, and a new one can start', () {
    down(1, const Offset(300, 300));
    down(2, const Offset(300, 400));
    swipe.onPointerCancel(const PointerCancelEvent(pointer: 2));
    clock.advance(50);
    move(1, const Offset(200, 300));
    expect(lefts, 0);
    up(1, const Offset(200, 300));
    twoFingers(-90, 0);
    expect(lefts, 1);
  });

  test('the window counts from the second finger', () {
    down(1, const Offset(300, 300));
    clock.advance(1000);
    down(2, const Offset(300, 400));
    clock.advance(100);
    move(1, const Offset(200, 300));
    move(2, const Offset(200, 400));
    expect(lefts, 1);
  });
}
