import 'dart:math' as math;

import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/updating_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

final _theme = claxTheme(Brightness.light, TargetPlatform.android);

/// The cover in the app's theme, animations reduced when [reduced].
Widget _app({
  required bool updating,
  String? title = 'Plant Diary',
  bool reduced = false,
  int seed = 1,
}) => MaterialApp(
  theme: _theme,
  home: Builder(
    builder: (context) => MediaQuery(
      data: MediaQuery.of(context).copyWith(disableAnimations: reduced),
      child: UpdatingCover(
        updating: updating,
        title: title,
        random: math.Random(seed),
        seed: seed,
        page: (_) => const Text('The page'),
      ),
    ),
  ),
);

/// The line coming in: the switcher's newest child.
String _line(WidgetTester tester) => tester
    .widgetList<Text>(
      find.descendant(
        of: find.byType(AnimatedSwitcher),
        matching: find.byType(Text),
      ),
    )
    .last
    .data!;

void main() {
  testWidgets('the first line names the app', (tester) async {
    await tester.pumpWidget(_app(updating: true));
    expect(_line(tester), 'Updating Plant Diary…');
    expect(find.text('The page'), findsNothing);
  });

  testWidgets('without a title the first line is Updating…', (tester) async {
    await tester.pumpWidget(_app(updating: true, title: null));
    expect(_line(tester), 'Updating…');
  });

  testWidgets(
    'lines turn every three seconds, every line in each round, none twice in a row',
    (tester) async {
      for (var seed = 0; seed < 12; seed++) {
        await tester.pumpWidget(_app(updating: true, seed: seed));
        await tester.pump();
        final shown = [_line(tester)];
        for (var i = 0; i < 3 * updatingLines.length; i++) {
          await tester.pump(
            UpdatingCover.lineEvery - const Duration(milliseconds: 1),
          );
          expect(_line(tester), shown.last, reason: 'not before three seconds');
          await tester.pump(const Duration(milliseconds: 1));
          shown.add(_line(tester));
        }
        expect(shown.first, 'Updating Plant Diary…');
        final turned = shown.skip(1).toList();
        for (var i = 1; i < turned.length; i++) {
          expect(turned[i], isNot(turned[i - 1]), reason: 'seed $seed at $i');
        }
        for (var r = 0; r < 3; r++) {
          final round = turned.sublist(
            r * updatingLines.length,
            (r + 1) * updatingLines.length,
          );
          expect(round.toSet(), updatingLines.toSet(), reason: 'seed $seed');
        }
        // A fresh showing for the next seed.
        await tester.pumpWidget(const SizedBox());
      }
    },
  );

  testWidgets('the finish plays its ta-da before the page shows', (
    tester,
  ) async {
    await tester.pumpWidget(_app(updating: true));
    await tester.pump(const Duration(milliseconds: 1500));
    await tester.pumpWidget(_app(updating: false));
    // The frame that starts the finish.
    await tester.pump(const Duration(milliseconds: 16));
    final hold = ClaxSynapse.finishLength - const Duration(milliseconds: 16);
    await tester.pump(hold);
    expect(find.text('The page'), findsNothing);
    expect(find.byType(ClaxSynapse), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 20));
    expect(find.text('The page'), findsOneWidget);
    expect(find.byType(ClaxSynapse), findsNothing);
  });

  testWidgets('an update that starts again during the finish keeps the cover', (
    tester,
  ) async {
    await tester.pumpWidget(_app(updating: true));
    await tester.pump(const Duration(milliseconds: 600));
    await tester.pumpWidget(_app(updating: false));
    await tester.pump(const Duration(milliseconds: 100));
    await tester.pumpWidget(_app(updating: true));
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('The page'), findsNothing);
    expect(find.byType(ClaxSynapse), findsOneWidget);
  });

  testWidgets(
    'under reduced motion the mark is still and the page comes at once',
    (tester) async {
      await tester.pumpWidget(_app(updating: true, reduced: true));
      await tester.pump();
      expect(find.byType(ClaxMark), findsOneWidget);
      expect(
        find.byWidgetPredicate(
          (w) => w is CustomPaint && w.painter is ClaxSynapsePainter,
        ),
        findsNothing,
      );
      expect(tester.binding.transientCallbackCount, 0, reason: 'nothing ticks');
      // The words still turn.
      await tester.pump(UpdatingCover.lineEvery);
      expect(_line(tester), isNot('Updating Plant Diary…'));
      await tester.pump(const Duration(seconds: 1));
      await tester.pumpWidget(_app(updating: false, reduced: true));
      expect(find.text('The page'), findsOneWidget);
    },
  );

  testWidgets('with motion the synapse ticks', (tester) async {
    await tester.pumpWidget(_app(updating: true));
    await tester.pump(const Duration(milliseconds: 100));
    expect(
      find.byWidgetPredicate(
        (w) => w is CustomPaint && w.painter is ClaxSynapsePainter,
      ),
      findsOneWidget,
    );
    expect(tester.binding.transientCallbackCount, greaterThan(0));
  });

  testWidgets('a screen reader hears one steady label, not the turning lines', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await tester.pumpWidget(_app(updating: true));
    for (var i = 0; i < 4; i++) {
      expect(
        tester.getSemantics(find.bySemanticsLabel('Updating Plant Diary')),
        isSemantics(label: 'Updating Plant Diary', isLiveRegion: false),
      );
      for (final line in updatingLines) {
        expect(find.bySemanticsLabel(line), findsNothing);
      }
      expect(find.bySemanticsLabel('Updating Plant Diary…'), findsNothing);
      await tester.pump(UpdatingCover.lineEvery);
    }
    semantics.dispose();
  });
}
