import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

final _themes = {
  for (final b in Brightness.values) b: claxTheme(b, TargetPlatform.android),
};

/// [child] in the app's [brightness] theme, with animations reduced when
/// [reduced]. The themes are built once, so a second pump changes no theme.
Widget _app(
  Widget child, {
  Brightness brightness = Brightness.light,
  bool reduced = false,
}) => MaterialApp(
  theme: _themes[brightness],
  home: Builder(
    builder: (context) => MediaQuery(
      data: MediaQuery.of(context).copyWith(disableAnimations: reduced),
      child: Scaffold(body: Center(child: child)),
    ),
  ),
);

/// The painter the one mark on screen draws with.
ClaxMarkPainter _painter(WidgetTester tester) {
  final paint = tester.widget<CustomPaint>(
    find.descendant(
      of: find.byType(ClaxMark),
      matching: find.byType(CustomPaint),
    ),
  );
  return paint.painter! as ClaxMarkPainter;
}

/// What a painter draws: its paths and circles, with their paints.
class _Drawn {
  final paths = <(Path, Paint)>[];
  final circles = <(Offset, double, Paint)>[];
}

_Drawn _draw(WidgetTester tester) {
  final drawn = _Drawn();
  expect(
    find.descendant(
      of: find.byType(ClaxMark),
      matching: find.byType(CustomPaint),
    ),
    paints..everything((method, args) {
      if (method == #drawPath) {
        drawn.paths.add((args[0] as Path, args[1] as Paint));
      } else if (method == #drawCircle) {
        drawn.circles.add((args[0] as Offset, args[1] as double, args[2] as Paint));
      }
      return true;
    }),
  );
  return drawn;
}

/// [r]'s edges, each to within [eps].
Matcher _rect(Rect r, {double eps = 1e-6}) => isA<Rect>()
    .having((x) => x.left, 'left', closeTo(r.left, eps))
    .having((x) => x.top, 'top', closeTo(r.top, eps))
    .having((x) => x.right, 'right', closeTo(r.right, eps))
    .having((x) => x.bottom, 'bottom', closeTo(r.bottom, eps));

void main() {
  group('ClaxMarkGeometry', () {
    test('is design/mark.json: a 22 by 16 box, a 2.6 round stroke', () {
      expect(ClaxMarkGeometry.width, 22);
      expect(ClaxMarkGeometry.height, 16);
      expect(ClaxMarkGeometry.stroke, 2.6);
      expect(ClaxMarkGeometry.cap, StrokeCap.round);
      expect(ClaxMarkGeometry.dot, const Offset(11, 8));
      expect(ClaxMarkGeometry.dotRadius, 1.7);
    });

    test('the arcs face the dot: ) · (', () {
      final you = ClaxMarkGeometry.you();
      final agent = ClaxMarkGeometry.agent();
      expect(you.getBounds(), _rect(const Rect.fromLTRB(1.5, 2.5, 7, 13.5), eps: 1e-3));
      expect(agent.getBounds(), _rect(const Rect.fromLTRB(15, 2.5, 20.5, 13.5), eps: 1e-3));
      // The person's arc bulges right, toward the dot; Clax's bulges left.
      expect(you.contains(const Offset(1.6, 8)), isTrue);
      expect(you.contains(const Offset(1.4, 8)), isFalse);
      expect(agent.contains(const Offset(20.4, 8)), isTrue);
      expect(agent.contains(const Offset(20.6, 8)), isFalse);
    });
  });

  group('ClaxMark', () {
    testWidgets('draws the three shapes of mark.json at its size', (tester) async {
      const size = 44.0;
      const s = size / 22;
      await tester.pumpWidget(_app(const ClaxMark(size: size)));
      expect(tester.getSize(find.byType(ClaxMark)), const Size(44, 32));
      final drawn = _draw(tester);
      expect(drawn.paths, hasLength(2));
      expect(drawn.circles, hasLength(1));
      final (you, youPaint) = drawn.paths[0];
      final (agent, agentPaint) = drawn.paths[1];
      Rect scaled(double l, double t, double r, double b) =>
          Rect.fromLTRB(l * s, t * s, r * s, b * s);
      expect(
        you.getBounds().inflate(youPaint.strokeWidth / 2),
        _rect(scaled(1.5 - 1.3, 2.5 - 1.3, 7 + 1.3, 13.5 + 1.3), eps: 1e-3),
      );
      expect(youPaint.strokeWidth, closeTo(2.6 * s, 1e-5));
      expect(youPaint.strokeCap, StrokeCap.round);
      expect(youPaint.style, PaintingStyle.stroke);
      expect(youPaint.color, isSameColorAs(ClaxTokens.light.you));
      expect(
        agent.getBounds().inflate(agentPaint.strokeWidth / 2),
        _rect(scaled(15 - 1.3, 2.5 - 1.3, 20.5 + 1.3, 13.5 + 1.3), eps: 1e-3),
      );
      expect(agentPaint.color, isSameColorAs(ClaxTokens.light.agent));
      final (centre, radius, dotPaint) = drawn.circles.single;
      expect(centre, const Offset(11 * s, 8 * s));
      expect(radius, closeTo(1.7 * s, 1e-9));
      expect(dotPaint.color, isSameColorAs(ClaxTokens.light.fg));
    });

    testWidgets('draws in the dark tokens on dark, and one colour when monochrome', (
      tester,
    ) async {
      await tester.pumpWidget(_app(const ClaxMark(), brightness: Brightness.dark));
      var drawn = _draw(tester);
      expect(drawn.paths[0].$2.color, isSameColorAs(ClaxTokens.dark.you));
      expect(drawn.paths[1].$2.color, isSameColorAs(ClaxTokens.dark.agent));
      expect(drawn.circles.single.$3.color, isSameColorAs(ClaxTokens.dark.fg));

      const ink = Color(0xFF123456);
      await tester.pumpWidget(_app(const ClaxMark(monochrome: ink)));
      drawn = _draw(tester);
      expect(drawn.paths[0].$2.color, isSameColorAs(ink));
      expect(drawn.paths[1].$2.color, isSameColorAs(ink));
      expect(drawn.circles.single.$3.color, isSameColorAs(ink));
    });

    testWidgets('resting is still', (tester) async {
      await tester.pumpWidget(_app(const ClaxMark()));
      expect(_painter(tester).dotShift, 0);
      await tester.pump(const Duration(milliseconds: 800));
      expect(_painter(tester).dotShift, 0);
      expect(_painter(tester).agentScale, 1);
      expect(tester.hasRunningAnimations, isFalse);
    });

    testWidgets('working breathes toward Clax and back every 1.6 s', (tester) async {
      await tester.pumpWidget(_app(const ClaxMark(state: ClaxMarkState.working)));
      expect(_painter(tester).dotShift, 0);
      await tester.pump(const Duration(milliseconds: 400));
      final quarter = _painter(tester).dotShift;
      expect(quarter, greaterThan(0.1));
      expect(quarter, lessThan(0.3));
      await tester.pump(const Duration(milliseconds: 400));
      expect(_painter(tester).dotShift, closeTo(0.35, 0.01));
      expect(_painter(tester).dotScale, greaterThan(1));
      await tester.pump(const Duration(milliseconds: 800));
      expect(_painter(tester).dotShift, closeTo(0, 0.01));
      await tester.pump(const Duration(milliseconds: 800));
      expect(_painter(tester).dotShift, closeTo(0.35, 0.01));
    });

    testWidgets('asking breathes toward the person', (tester) async {
      await tester.pumpWidget(_app(const ClaxMark(state: ClaxMarkState.asking)));
      await tester.pump(const Duration(milliseconds: 800));
      expect(_painter(tester).dotShift, closeTo(-0.35, 0.01));
    });

    testWidgets('sent slides the dot to Clax once, in 300 ms', (tester) async {
      await tester.pumpWidget(_app(const ClaxMark(state: ClaxMarkState.sent)));
      var furthest = 0.0;
      for (var t = 0; t < 300; t += 20) {
        await tester.pump(const Duration(milliseconds: 20));
        final shift = _painter(tester).dotShift;
        expect(shift, greaterThanOrEqualTo(-1e-9));
        if (shift > furthest) furthest = shift;
      }
      expect(furthest, closeTo(1, 0.05));
      await tester.pump(const Duration(milliseconds: 20));
      expect(_painter(tester).dotShift, 0);
      expect(tester.hasRunningAnimations, isFalse);
    });

    testWidgets('landed nods Clax’s arc to 1.08 and back, in 300 ms', (tester) async {
      await tester.pumpWidget(_app(const ClaxMark(state: ClaxMarkState.landed)));
      var biggest = 1.0;
      for (var t = 0; t < 300; t += 10) {
        await tester.pump(const Duration(milliseconds: 10));
        final scale = _painter(tester).agentScale;
        if (scale > biggest) biggest = scale;
      }
      expect(biggest, closeTo(1.08, 0.005));
      await tester.pump(const Duration(milliseconds: 10));
      expect(_painter(tester).agentScale, 1);
      expect(_painter(tester).dotShift, 0);
      expect(tester.hasRunningAnimations, isFalse);
    });

    testWidgets('rally bounces the dot off the arcs ten times, then rests', (
      tester,
    ) async {
      await tester.pumpWidget(_app(const ClaxMark(state: ClaxMarkState.rally)));
      var hits = 0;
      var side = 0;
      for (var t = 0; t < 4000; t += 5) {
        await tester.pump(const Duration(milliseconds: 5));
        final shift = _painter(tester).dotShift;
        final now = shift > 0.9 ? 1 : (shift < -0.9 ? -1 : 0);
        if (now != 0 && now != side) {
          hits++;
          expect(now, hits.isOdd ? 1 : -1, reason: 'hit $hits');
        }
        if (now != 0) side = now;
      }
      expect(hits, 10);
      expect(_painter(tester).dotShift, 0);
      expect(tester.hasRunningAnimations, isFalse);
    });

    testWidgets('a change of state glides from where the dot was', (tester) async {
      await tester.pumpWidget(_app(const ClaxMark(state: ClaxMarkState.working)));
      await tester.pump(const Duration(milliseconds: 800));
      expect(_painter(tester).dotShift, closeTo(0.35, 0.01));
      await tester.pumpWidget(_app(const ClaxMark()));
      await tester.pump(const Duration(milliseconds: 16));
      final shift = _painter(tester).dotShift;
      expect(shift, greaterThan(0.05));
      expect(shift, lessThan(0.33));
      await tester.pump(const Duration(milliseconds: 300));
      expect(_painter(tester).dotShift, 0);
      expect(tester.hasRunningAnimations, isFalse);
    });

    for (final state in ClaxMarkState.values) {
      testWidgets('with animations reduced, ${state.name} draws still', (tester) async {
        await tester.pumpWidget(_app(ClaxMark(state: state), reduced: true));
        final first = _painter(tester);
        for (var t = 0; t < 4; t++) {
          await tester.pump(const Duration(milliseconds: 400));
          final now = _painter(tester);
          expect(now.dotShift, first.dotShift);
          expect(now.agentScale, first.agentScale);
          expect(now.youScale, first.youScale);
          expect(now.dotScale, first.dotScale);
        }
        expect(tester.hasRunningAnimations, isFalse);
        // A still mark that waits still says which way it leans.
        expect(first.dotShift, switch (state) {
          ClaxMarkState.working => closeTo(0.35, 1e-9),
          ClaxMarkState.asking => closeTo(-0.35, 1e-9),
          _ => 0,
        });
      });

      testWidgets('quiet, ${state.name} draws still', (tester) async {
        await tester.pumpWidget(
          _app(ClaxQuiet(quiet: true, child: ClaxMark(state: state))),
        );
        final first = _painter(tester).dotShift;
        await tester.pump(const Duration(milliseconds: 800));
        expect(_painter(tester).dotShift, first);
        expect(tester.hasRunningAnimations, isFalse);
      });
    }

    testWidgets('a mark that quiets mid-breath stops, and breathes again after', (
      tester,
    ) async {
      Widget at(bool quiet) => _app(
        ClaxQuiet(
          quiet: quiet,
          child: const ClaxMark(state: ClaxMarkState.working),
        ),
      );
      await tester.pumpWidget(at(false));
      await tester.pump(const Duration(milliseconds: 400));
      await tester.pumpWidget(at(true));
      await tester.pump(const Duration(milliseconds: 400));
      expect(tester.hasRunningAnimations, isFalse);
      final still = _painter(tester).dotShift;
      await tester.pump(const Duration(milliseconds: 400));
      expect(_painter(tester).dotShift, still);
      await tester.pumpWidget(at(false));
      await tester.pump(const Duration(milliseconds: 400));
      expect(tester.hasRunningAnimations, isTrue);
    });

    testWidgets('ClaxQuiet.of reads the nearest one', (tester) async {
      late bool outer, inner;
      await tester.pumpWidget(
        Builder(
          builder: (c) {
            outer = ClaxQuiet.of(c);
            return ClaxQuiet(
              quiet: true,
              child: Builder(
                builder: (c) {
                  inner = ClaxQuiet.of(c);
                  return const SizedBox();
                },
              ),
            );
          },
        ),
      );
      expect(outer, isFalse);
      expect(inner, isTrue);
    });

    testWidgets('says its state to screen readers', (tester) async {
      final handle = tester.ensureSemantics();
      const words = {
        ClaxMarkState.working: 'Clax is working',
        ClaxMarkState.sent: 'Sent to Clax',
        ClaxMarkState.landed: 'New version ready',
        ClaxMarkState.asking: 'Clax has a question',
        ClaxMarkState.rally: 'New version ready',
      };
      for (final MapEntry(key: state, value: label) in words.entries) {
        await tester.pumpWidget(_app(ClaxMark(state: state)));
        expect(find.bySemanticsLabel(label), findsOneWidget, reason: state.name);
      }
      await tester.pumpWidget(_app(const ClaxMark()));
      expect(find.bySemanticsLabel(RegExp('Clax|Sent|version')), findsNothing);
      await tester.pumpWidget(
        _app(const ClaxMark(state: ClaxMarkState.working, semanticsLabel: 'Making Groceries')),
      );
      expect(find.bySemanticsLabel('Making Groceries'), findsOneWidget);
      expect(find.bySemanticsLabel('Clax is working'), findsNothing);
      handle.dispose();
    });

    test('defaultLabel names each state', () {
      expect(ClaxMark.defaultLabel(ClaxMarkState.resting), isNull);
      expect(ClaxMark.defaultLabel(ClaxMarkState.working), 'Clax is working');
      expect(ClaxMark.defaultLabel(ClaxMarkState.sent), 'Sent to Clax');
      expect(ClaxMark.defaultLabel(ClaxMarkState.landed), 'New version ready');
      expect(ClaxMark.defaultLabel(ClaxMarkState.asking), 'Clax has a question');
    });
  });
}
