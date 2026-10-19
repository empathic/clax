import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// [child] in the app's light theme on [platform], under the root pointer
/// scope.
Widget _app(Widget child, {TargetPlatform platform = TargetPlatform.android}) =>
    ClaxPointerScope(
      child: MaterialApp(
        theme: claxTheme(Brightness.light, platform),
        home: Scaffold(body: child),
      ),
    );

/// Sizes the test's window [width] wide.
Future<void> _wide(WidgetTester tester, double width) async {
  tester.view.physicalSize = Size(width, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
}

void main() {
  group('ClaxBadge', () {
    test('counts to 99, then 99+, and says nothing at 0', () {
      expect(ClaxBadge.text(0), '');
      expect(ClaxBadge.text(1), '1');
      expect(ClaxBadge.text(99), '99');
      expect(ClaxBadge.text(100), '99+');
    });

    testWidgets('a 0 badge draws no text', (tester) async {
      await tester.pumpWidget(
        _app(const ClaxBadge(count: 0, child: Icon(Icons.inbox))),
      );
      expect(
        find.descendant(of: find.byType(ClaxBadge), matching: find.byType(Text)),
        findsNothing,
      );
      expect(find.byIcon(Icons.inbox), findsOneWidget);
    });

    testWidgets('a badge is you on on-you', (tester) async {
      await tester.pumpWidget(_app(const ClaxBadge(count: 120)));
      expect(find.text('99+'), findsOneWidget);
      final badge = tester.widget<Badge>(find.byType(Badge));
      expect(badge.backgroundColor, ClaxTokens.light.you);
      expect(badge.textColor, ClaxTokens.light.onYou);
    });
  });

  group('ClaxButton', () {
    testWidgets('on Android it is 40 high in a 48 by 48 target', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(
          Center(
            child: ClaxButton(
              label: 'OK',
              onPressed: () {},
              kind: ClaxButtonKind.primary,
            ),
          ),
        ),
      );
      final target = tester.getSize(find.byType(ClaxButton));
      expect(target.height, greaterThanOrEqualTo(48));
      expect(target.width, greaterThanOrEqualTo(48));
      final visual = tester.getSize(
        find.descendant(
          of: find.byType(ClaxButton),
          matching: find.byType(Material),
        ),
      );
      expect(visual.height, 40);
    });

    testWidgets('destructive paints its label in danger', (tester) async {
      await tester.pumpWidget(
        _app(
          ClaxButton(
            label: 'Delete',
            onPressed: () {},
            kind: ClaxButtonKind.destructive,
          ),
        ),
      );
      final style = DefaultTextStyle.of(
        tester.element(find.text('Delete')),
      ).style;
      expect(style.color, ClaxTokens.light.danger);
    });

    testWidgets('tonal is Clax\'s tint, not the ink of the filled theme', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(
          ClaxButton(
            label: 'Open thread',
            onPressed: () {},
            kind: ClaxButtonKind.tonal,
          ),
        ),
      );
      final material = tester.widget<Material>(
        find.descendant(
          of: find.byType(ClaxButton),
          matching: find.byType(Material),
        ),
      );
      expect(material.color, ClaxTokens.light.accentTint);
      final style = DefaultTextStyle.of(
        tester.element(find.text('Open thread')),
      ).style;
      expect(style.color, ClaxTokens.light.agentInk);
    });

    testWidgets('on the Mac it is a 24 pt push button, the default in the '
        'accent', (tester) async {
      await tester.pumpWidget(
        _app(
          Center(
            child: ClaxButton(label: 'Save', onPressed: () {}, isDefault: true),
          ),
          platform: TargetPlatform.macOS,
        ),
      );
      expect(tester.getSize(find.byType(ClaxButton)).height, 24);
      final material = tester.widget<Material>(
        find.descendant(
          of: find.byType(ClaxButton),
          matching: find.byType(Material),
        ),
      );
      expect(material.color, ClaxTokens.light.agent);
    });

    testWidgets('a press calls onPressed', (tester) async {
      var pressed = 0;
      await tester.pumpWidget(
        _app(ClaxButton(label: 'Go', onPressed: () => pressed++)),
      );
      await tester.tap(find.text('Go'));
      expect(pressed, 1);
    });
  });

  group('ClaxTextField', () {
    testWidgets('carries its label and submits', (tester) async {
      String? submitted;
      final controller = TextEditingController();
      addTearDown(controller.dispose);
      for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
        await tester.pumpWidget(
          _app(
            ClaxTextField(
              controller: controller,
              label: 'API key',
              onSubmitted: (v) => submitted = v,
            ),
            platform: platform,
          ),
        );
        expect(find.text('API key'), findsOneWidget);
        await tester.enterText(find.byType(TextField), 'sk-$platform');
        await tester.testTextInput.receiveAction(TextInputAction.done);
        expect(submitted, 'sk-$platform');
      }
    });
  });

  group('toggles', () {
    testWidgets('a labelled switch toggles from its label', (tester) async {
      bool? got;
      for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
        got = null;
        await tester.pumpWidget(
          _app(
            ClaxSwitch(value: false, onChanged: (v) => got = v, label: 'Sync'),
            platform: platform,
          ),
        );
        await tester.tap(find.text('Sync'));
        expect(got, isTrue, reason: '$platform');
      }
    });

    testWidgets('a checkbox toggles from its label', (tester) async {
      bool? got;
      for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
        got = null;
        await tester.pumpWidget(
          _app(
            ClaxCheckbox(
              value: true,
              onChanged: (v) => got = v,
              label: 'Always for this app',
            ),
            platform: platform,
          ),
        );
        await tester.tap(find.text('Always for this app'));
        expect(got, isFalse, reason: '$platform');
      }
    });

    testWidgets('a radio picks its value', (tester) async {
      String? got;
      for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
        got = null;
        await tester.pumpWidget(
          _app(
            Column(
              children: [
                ClaxRadio<String>(
                  value: 'precise',
                  group: 'approximate',
                  onChanged: (v) => got = v,
                  label: 'Precise',
                  detail: 'Your exact spot',
                ),
                ClaxRadio<String>(
                  value: 'approximate',
                  group: 'approximate',
                  onChanged: (v) => got = v,
                  label: 'Approximate',
                ),
              ],
            ),
            platform: platform,
          ),
        );
        await tester.tap(find.text('Your exact spot'));
        expect(got, 'precise', reason: '$platform');
      }
    });
  });

  group('ClaxEmpty', () {
    testWidgets('is centred text at most 360 wide', (tester) async {
      await _wide(tester, 1200);
      await tester.pumpWidget(
        _app(
          ClaxEmpty(
            icon: Icons.inbox_outlined,
            title: 'All caught up.',
            body: 'Comments people send to Clax show up here.',
            actions: [ClaxButton(label: 'Back to Home', onPressed: () {})],
          ),
        ),
      );
      for (final t in ['All caught up.',
        'Comments people send to Clax show up here.']) {
        expect(tester.widget<Text>(find.text(t)).textAlign, TextAlign.center);
      }
      final column = find.descendant(
        of: find.byType(ClaxEmpty),
        matching: find.byType(Column),
      );
      expect(tester.getSize(column.first).width, lessThanOrEqualTo(360));
    });
  });

  group('ClaxBanner', () {
    Widget banner() => ClaxBanner(
      icon: Icons.key_outlined,
      title: 'Add your API key',
      body: 'Clax needs it to make apps.',
      actions: [ClaxButton(label: 'Add key', onPressed: () {})],
    );

    testWidgets('puts its actions on the title\'s row when wide', (
      tester,
    ) async {
      await _wide(tester, 900);
      await tester.pumpWidget(_app(banner()));
      final title = tester.getRect(find.text('Add your API key'));
      final action = tester.getRect(find.byType(ClaxButton));
      expect(action.top, lessThan(title.bottom));
      expect(action.left, greaterThan(title.right));
    });

    testWidgets('puts its actions below the text when compact', (
      tester,
    ) async {
      await _wide(tester, 400);
      await tester.pumpWidget(_app(banner()));
      final body = tester.getRect(find.text('Clax needs it to make apps.'));
      final action = tester.getRect(find.byType(ClaxButton));
      expect(action.top, greaterThanOrEqualTo(body.bottom));
    });
  });

  group('ClaxVerbs', () {
    Widget probe(List<ClaxVerbs> seen) => Builder(
      builder: (context) {
        final v = ClaxVerbs.of(context);
        seen.add(v);
        return SizedBox.expand(child: Text(v.tap));
      },
    );

    testWidgets('says Tap on Android until a mouse moves, then Click', (
      tester,
    ) async {
      final seen = <ClaxVerbs>[];
      await tester.pumpWidget(_app(probe(seen)));
      expect(find.text('Tap'), findsOneWidget);
      await tester.tap(find.text('Tap'));
      await tester.pump();
      expect(find.text('Tap'), findsOneWidget);

      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      addTearDown(mouse.removePointer);
      await mouse.addPointer(location: const Offset(10, 10));
      await mouse.moveTo(const Offset(20, 20));
      await tester.pump();
      expect(find.text('Click'), findsOneWidget);
      expect(seen.last, ClaxVerbs.mouse);
      expect(seen.last.longPress, 'Right-click');
      expect(seen.last.swipe, 'Drag');

      await tester.tap(find.text('Click'));
      await tester.pump();
      expect(find.text('Tap'), findsOneWidget);
      expect(seen.last.longPress, 'Long-press');
      expect(seen.last.swipe, 'Swipe');
    });

    testWidgets('says Click on the Mac before any pointer', (tester) async {
      await tester.pumpWidget(
        _app(probe([]), platform: TargetPlatform.macOS),
      );
      expect(find.text('Click'), findsOneWidget);
    });

    testWidgets('says Tap on iOS before any pointer', (tester) async {
      await tester.pumpWidget(_app(probe([]), platform: TargetPlatform.iOS));
      expect(find.text('Tap'), findsOneWidget);
    });
  });
}
