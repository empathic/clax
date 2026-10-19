import 'dart:async';

import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/kit/mark.dart';
import 'package:clax_app/ui/splash.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A clock that never moves by itself: its delays finish on [elapse].
class _ManualClock implements Clock {
  final _waits = <(Duration, Completer<void>)>[];
  Duration _now = Duration.zero;

  @override
  DateTime now() => DateTime.fromMillisecondsSinceEpoch(_now.inMilliseconds);

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _waits.add((_now + d, c));
    return c.future;
  }

  void elapse(Duration d) {
    _now += d;
    for (final w in [..._waits]) {
      if (w.$1 <= _now) {
        _waits.remove(w);
        w.$2.complete();
      }
    }
  }
}

final _themes = {
  for (final b in Brightness.values) b: claxTheme(b, TargetPlatform.android),
};

Widget _app(Widget child) =>
    MaterialApp(theme: _themes[Brightness.light], home: child);

ClaxMarkState _state(WidgetTester tester) =>
    tester.widget<ClaxMark>(find.byType(ClaxMark)).state;

/// The opacity "Starting…" is drawn at, or null when it is not built.
double? _startingOpacity(WidgetTester tester) {
  final text = find.text('Starting…');
  if (text.evaluate().isEmpty) return null;
  final fade = tester.widget<FadeTransition>(
    find.ancestor(of: text, matching: find.byType(FadeTransition)).first,
  );
  return fade.opacity.value;
}

void main() {
  testWidgets('rests at first, with no "Starting…"', (tester) async {
    await tester.pumpWidget(_app(const ClaxSplash()));
    expect(find.byType(ClaxMark), findsOneWidget);
    expect(_state(tester), ClaxMarkState.resting);
    expect(_startingOpacity(tester) ?? 0, 0);
    await tester.pump(const Duration(milliseconds: 599));
    expect(_state(tester), ClaxMarkState.resting);
    expect(_startingOpacity(tester) ?? 0, 0);
    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
  });

  testWidgets('breathes after 600 ms, and "Starting…" fades in under it', (
    tester,
  ) async {
    await tester.pumpWidget(_app(const ClaxSplash()));
    await tester.pump(const Duration(milliseconds: 600));
    expect(_state(tester), ClaxMarkState.working);
    expect(find.text('Starting…'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 400));
    expect(_startingOpacity(tester), 1);
    final mark = tester.getRect(find.byType(ClaxMark));
    final text = tester.getRect(find.text('Starting…'));
    expect(text.top, greaterThan(mark.bottom));
    expect(text.center.dx, closeTo(mark.center.dx, 0.5));
    // The mark loops; take it away so the test ends with nothing ticking.
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('waits on its clock, not on the frames', (tester) async {
    final clock = _ManualClock();
    await tester.pumpWidget(_app(ClaxSplash(clock: clock)));
    await tester.pump(const Duration(seconds: 2));
    expect(_state(tester), ClaxMarkState.resting);
    clock.elapse(const Duration(milliseconds: 599));
    await tester.pump();
    expect(_state(tester), ClaxMarkState.resting);
    clock.elapse(const Duration(milliseconds: 1));
    // The delay's callback runs in the first pump; its rebuild in the next.
    await tester.pump();
    await tester.pump();
    expect(_state(tester), ClaxMarkState.working);
    expect(find.text('Starting…'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('honours breatheAfter', (tester) async {
    await tester.pumpWidget(
      _app(const ClaxSplash(breatheAfter: Duration(milliseconds: 100))),
    );
    await tester.pump(const Duration(milliseconds: 100));
    expect(_state(tester), ClaxMarkState.working);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('draws the mark 132 wide at the window\'s centre, before and '
      'after the breath', (tester) async {
    tester.view.physicalSize = const Size(412, 915) * 2.625;
    tester.view.devicePixelRatio = 2.625;
    tester.view.padding = const FakeViewPadding(top: 24 * 2.625);
    addTearDown(tester.view.reset);
    await tester.pumpWidget(_app(const ClaxSplash()));
    const centre = Offset(412 / 2, 915 / 2);
    var mark = tester.getRect(find.byType(ClaxMark));
    expect(mark.width, 132);
    expect(mark.height, closeTo(132 * 16 / 22, 1e-9));
    expect(mark.center, centre);
    await tester.pump(const Duration(milliseconds: 700));
    mark = tester.getRect(find.byType(ClaxMark));
    expect(mark.center, centre);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('says no "Clax" and spins no progress indicator', (tester) async {
    await tester.pumpWidget(_app(const ClaxSplash()));
    await tester.pump(const Duration(milliseconds: 700));
    final splash = find.byType(ClaxSplash);
    expect(
      find.descendant(of: splash, matching: find.text('Clax')),
      findsNothing,
    );
    expect(
      find.descendant(
        of: splash,
        matching: find.byWidgetPredicate((w) => w is ProgressIndicator),
      ),
      findsNothing,
    );
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('stops waiting once it is gone', (tester) async {
    await tester.pumpWidget(_app(const ClaxSplash()));
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 600));
    expect(tester.takeException(), isNull);
  });
}
