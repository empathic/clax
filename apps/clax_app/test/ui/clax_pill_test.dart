import 'dart:async';

import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/kit/mark.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

/// A clock whose delays end only when [lapse] ends them.
class _Clock implements Clock {
  final _delays = <(Duration, Completer<void>)>[];
  @override
  DateTime now() => DateTime.utc(2026, 10, 19);
  @override
  Future<void> delay(Duration d) =>
      (_delays..add((d, Completer<void>()))).last.$2.future;

  /// Ends every delay asked for so far that is no longer than [upTo].
  void lapse([Duration upTo = const Duration(days: 1)]) {
    for (final (d, c) in List.of(_delays)) {
      if (d <= upTo) {
        c.complete();
        _delays.removeWhere((e) => identical(e.$2, c));
      }
    }
  }
}

final _pill = find.byKey(const ValueKey('clax-pill'));

/// Past the fold's resize; the breathing mark never settles.
Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 4; i++) {
    await tester.pump(const Duration(milliseconds: 100));
  }
}

void main() {
  late _Clock clock;
  late List<String> log;

  setUp(() {
    clock = _Clock();
    log = [];
  });

  Future<void> pump(
    WidgetTester tester,
    ClaxPillState? state, {
    ({int done, int total})? progress,
    double textScale = 1,
    Brightness brightness = Brightness.light,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(brightness, TargetPlatform.android),
        home: MediaQuery.withClampedTextScaling(
          minScaleFactor: textScale,
          maxScaleFactor: textScale,
          child: Scaffold(
            body: Align(
              alignment: Alignment.topRight,
              child: ClaxPill(
                state: state,
                progress: progress,
                clock: clock,
                onTap: () => log.add('tap'),
                onShow: () => log.add('show'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets('it shows its label, then folds to a 32 dp circle after 3 s', (
    tester,
  ) async {
    await pump(tester, ClaxPillState.working);
    expect(find.text('Working'), findsOneWidget);
    expect(tester.getSize(_pill).height, 32);
    expect(tester.getSize(_pill).width, greaterThan(64));
    clock.lapse(const Duration(seconds: 3));
    await settle(tester);
    expect(find.text('Working'), findsNothing);
    expect(tester.getSize(_pill), const Size(32, 32));
    expect(find.byType(ClaxMark), findsOneWidget);
  });

  testWidgets('a new state or progress opens it again', (tester) async {
    await pump(tester, ClaxPillState.building, progress: (done: 2, total: 5));
    expect(find.text('Building 2/5'), findsOneWidget);
    clock.lapse();
    await settle(tester);
    expect(find.text('Building 2/5'), findsNothing);
    await pump(tester, ClaxPillState.building, progress: (done: 3, total: 5));
    await settle(tester);
    expect(find.text('Building 3/5'), findsOneWidget);
    clock.lapse();
    await settle(tester);
    await pump(tester, ClaxPillState.newVersion);
    await settle(tester);
    expect(find.text('New version'), findsOneWidget);
    expect(find.text('Show'), findsOneWidget);
  });

  testWidgets('nothing shows while nothing goes on', (tester) async {
    await pump(tester, null);
    expect(_pill, findsNothing);
    expect(find.byType(ClaxMark), findsNothing);
  });

  testWidgets('its target is 48 × 48 dp, folded too', (tester) async {
    await pump(tester, ClaxPillState.location);
    clock.lapse();
    await settle(tester);
    final target = find.byKey(const ValueKey('clax-pill-target'));
    expect(tester.getSize(target).height, greaterThanOrEqualTo(48));
    expect(tester.getSize(target).width, greaterThanOrEqualTo(48));
    await expectLater(tester, meetsGuideline(androidTapTargetGuideline));
    // The target's edge, outside the pill seen, still takes the tap.
    await tester.tapAt(tester.getRect(target).bottomLeft + const Offset(2, -2));
    expect(log, ['tap']);
  });

  testWidgets('a tap opens what it says; New version’s tap is Show', (
    tester,
  ) async {
    await pump(tester, ClaxPillState.working);
    await tester.tap(_pill);
    await pump(tester, ClaxPillState.newVersion);
    await tester.tap(_pill);
    expect(log, ['tap', 'show']);
  });

  testWidgets('at 200% text the label is whole', (tester) async {
    for (final (state, text) in [
      (ClaxPillState.working, 'Working'),
      (ClaxPillState.building, 'Building 12/15'),
      (ClaxPillState.newVersion, 'New version'),
      (ClaxPillState.location, 'Location'),
    ]) {
      await pump(
        tester,
        state,
        progress: (done: 12, total: 15),
        textScale: 2,
      );
      await settle(tester);
      final label = tester.renderObject<RenderParagraph>(find.text(text));
      expect(label.didExceedMaxLines, isFalse, reason: text);
      expect(
        tester.getSize(find.text(text)).width,
        closeTo(label.getMaxIntrinsicWidth(double.infinity), 0.5),
        reason: '$text is not cut',
      );
      expect(
        tester.getRect(_pill).contains(tester.getRect(find.text(text)).center),
        isTrue,
      );
    }
  });

  testWidgets('a screen reader hears each state', (tester) async {
    final semantics = tester.ensureSemantics();
    for (final (state, words) in [
      (ClaxPillState.working, 'Clax is working'),
      (ClaxPillState.building, 'Building 2/5'),
      (ClaxPillState.newVersion, 'New version ready'),
      (ClaxPillState.location, 'Location in use'),
    ]) {
      await pump(tester, state, progress: (done: 2, total: 5));
      final data = tester.getSemantics(_pill.first);
      expect(
        find.semantics.byLabel(words),
        findsOneWidget,
        reason: '$state: ${data.label}',
      );
      final node = find.semantics.byLabel(words).evaluate().single;
      expect(node.getSemanticsData().flagsCollection.isLiveRegion, isTrue);
    }
    semantics.dispose();
  });

  testWidgets('its fill is fg and its words bg, in both themes', (
    tester,
  ) async {
    for (final b in Brightness.values) {
      await pump(tester, ClaxPillState.working, brightness: b);
      final p = claxPalette(tester.element(_pill));
      final box = tester.widget<DecoratedBox>(
        find.descendant(of: _pill, matching: find.byType(DecoratedBox)).first,
      );
      expect((box.decoration as BoxDecoration).color, p.fg);
      expect(tester.widget<Text>(find.text('Working')).style?.color, p.bg);
    }
  });
}
