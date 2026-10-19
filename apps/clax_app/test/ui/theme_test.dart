import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:clax_app/ui/version_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('the scheme is the brand, by hand', () {
    final t = claxTheme(Brightness.light, TargetPlatform.android);
    expect(t.colorScheme.primary, ClaxTokens.light.agent);
    expect(t.colorScheme.tertiary, ClaxTokens.light.you);
    expect(t.colorScheme.surface, ClaxTokens.light.bg);
    expect(
      t.colorScheme.surfaceContainerHighest,
      ClaxTokens.light.surfaceContainers[4],
    );
    expect(t.colorScheme.surfaceTint, Colors.transparent);
    expect(
      claxTheme(Brightness.dark, TargetPlatform.android).colorScheme.onError,
      const Color(0xFF141413),
    );
  });

  test('the dark scheme is the dark palette, and more contrast its own', () {
    final d = claxTheme(Brightness.dark, TargetPlatform.android).colorScheme;
    expect(d.brightness, Brightness.dark);
    expect(d.surface, ClaxTokens.dark.bg);
    expect(d.primary, ClaxTokens.dark.agent);
    expect(d.secondary, ClaxTokens.dark.fg);
    expect(d.inversePrimary, const Color(0xFF2F6F2A));
    final hc = claxTheme(
      Brightness.light,
      TargetPlatform.android,
      highContrast: true,
    );
    expect(hc.colorScheme.outlineVariant, ClaxTokens.lightMore.border);
  });

  test('fields are outlined and filled with card', () {
    final d = claxTheme(
      Brightness.light,
      TargetPlatform.android,
    ).inputDecorationTheme;
    expect(d.enabledBorder, isA<OutlineInputBorder>());
    expect(
      (d.enabledBorder! as OutlineInputBorder).borderSide.color,
      ClaxTokens.light.borderStrong,
    );
    expect((d.focusedBorder! as OutlineInputBorder).borderSide.width, 2);
    expect(d.fillColor, ClaxTokens.light.card);
  });

  test('the primary button is ink', () {
    final s = claxTheme(
      Brightness.light,
      TargetPlatform.android,
    ).filledButtonTheme.style!;
    expect(s.backgroundColor!.resolve({}), ClaxTokens.light.fg);
    expect(s.foregroundColor!.resolve({}), ClaxTokens.light.bg);
    expect(
      s.backgroundColor!.resolve({WidgetState.hovered}),
      ClaxTokens.light.primaryHover,
    );
  });

  test('Android pages go back predictively', () {
    expect(
      claxTheme(
        Brightness.light,
        TargetPlatform.android,
      ).pageTransitionsTheme.builders[TargetPlatform.android],
      isA<PredictiveBackPageTransitionsBuilder>(),
    );
  });

  test('the Apple family is compact with SF sizes', () {
    final t = claxTheme(Brightness.light, TargetPlatform.macOS);
    expect(t.visualDensity, VisualDensity.compact);
    expect(t.textTheme.bodyMedium!.fontSize, 13);
    expect(
      claxTheme(Brightness.light, TargetPlatform.android).visualDensity,
      VisualDensity.standard,
    );
  });

  test('the Material scale maps onto the roles', () {
    final t = claxTextTheme(TargetPlatform.android, ClaxTokens.light);
    expect(t.displaySmall!.fontSize, 28);
    expect(t.headlineSmall!.fontSize, 22);
    expect(t.titleLarge!.fontSize, 22);
    expect(t.titleMedium!.fontSize, 17);
    expect(t.titleSmall!.fontSize, 15);
    expect(t.bodyLarge!.fontSize, 16);
    expect(t.bodyMedium!.fontSize, 14);
    expect(t.bodySmall!.fontSize, 12);
    expect(t.labelSmall!.fontSize, 12);
    expect(t.bodyMedium!.color, ClaxTokens.light.fg);
    expect(t.bodyMedium!.letterSpacing, 0);
  });

  test('switches, checkboxes and badges take the brand', () {
    final t = claxTheme(Brightness.light, TargetPlatform.android);
    expect(
      t.switchTheme.trackColor!.resolve({WidgetState.selected}),
      ClaxTokens.light.agent,
    );
    expect(
      t.checkboxTheme.fillColor!.resolve({WidgetState.selected}),
      ClaxTokens.light.agent,
    );
    expect(t.badgeTheme.backgroundColor, ClaxTokens.light.you);
    expect(t.badgeTheme.textColor, ClaxTokens.light.onYou);
    expect(t.snackBarTheme.behavior, SnackBarBehavior.floating);
    expect(t.snackBarTheme.width, isNull);
    expect(t.snackBarTheme.insetPadding?.left, 16);
    expect(t.snackBarTheme.insetPadding?.right, 16);
    expect(
      t.listTileTheme.contentPadding,
      const EdgeInsets.symmetric(horizontal: 16),
    );
    expect(t.bottomSheetTheme.backgroundColor, ClaxTokens.light.raised);
    expect(t.bottomSheetTheme.showDragHandle, isTrue);
    expect(t.extension<ClaxColors>()!.palette, ClaxTokens.light);
  });

  test('diff colours are the agent and danger at 14% over card', () {
    for (final (b, p) in [
      (Brightness.light, ClaxTokens.light),
      (Brightness.dark, ClaxTokens.dark),
    ]) {
      final c = DiffColors.of(claxTheme(b, TargetPlatform.android));
      expect(c.add, Color.alphaBlend(p.agent.withValues(alpha: 0.14), p.card));
      expect(c.del, Color.alphaBlend(p.danger.withValues(alpha: 0.14), p.card));
      expect(c.addInk, p.agentInk);
      expect(c.onAdd, p.fg);
      expect(c.onDel, p.fg);
    }
  });

  test('two builds of one theme are equal', () {
    for (final b in Brightness.values) {
      expect(
        claxTheme(b, TargetPlatform.android),
        claxTheme(b, TargetPlatform.android),
      );
      expect(
        claxTheme(b, TargetPlatform.macOS, highContrast: true),
        claxTheme(b, TargetPlatform.macOS, highContrast: true),
      );
    }
    expect(
      claxTheme(Brightness.light, TargetPlatform.android),
      isNot(claxTheme(Brightness.dark, TargetPlatform.android)),
    );
  });

  testWidgets('rebuilding with a fresh, equal theme starts no animation', (
    tester,
  ) async {
    Widget app() => MaterialApp(
      theme: claxTheme(Brightness.light, TargetPlatform.android),
      home: const SizedBox(),
    );
    await tester.pumpWidget(app());
    await tester.pumpAndSettle();
    await tester.pumpWidget(app());
    expect(tester.hasRunningAnimations, isFalse);
  });

  testWidgets('claxPalette reads the theme\'s palette', (tester) async {
    late ClaxPalette got;
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.dark, TargetPlatform.android),
        home: Builder(
          builder: (context) {
            got = claxPalette(context);
            return const SizedBox();
          },
        ),
      ),
    );
    expect(got, ClaxTokens.dark);
  });

  testWidgets('a filled field\'s label sits in the outline\'s gap', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, TargetPlatform.android),
        home: Scaffold(
          body: TextField(
            controller: TextEditingController(text: 'sk-ant'),
            decoration: const InputDecoration(labelText: 'API key'),
          ),
        ),
      ),
    );
    final label = tester.getRect(find.text('API key'));
    final field = tester.getRect(find.byType(InputDecorator));
    expect(label.center.dy, closeTo(field.top, 2));
  });
}
