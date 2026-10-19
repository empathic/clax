import 'dart:async';

import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/kit/button.dart';
import 'package:clax_app/ui/kit/dialog.dart';
import 'package:clax_app/ui/kit/menu.dart';
import 'package:clax_app/ui/kit/scaffold.dart';
import 'package:clax_app/ui/kit/sheet.dart';
import 'package:clax_app/ui/kit/toast.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// Sizes the test's window to [size] logical pixels, with [top] of status
/// bar.
void window(WidgetTester tester, Size size, {double top = 0}) {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  tester.view.padding = FakeViewPadding(top: top);
  tester.view.viewPadding = FakeViewPadding(top: top);
  addTearDown(tester.view.reset);
}

/// A page in the brand's theme on [platform] with one button, "Go", that
/// runs [onGo] with the button's context.
Widget page(
  void Function(BuildContext context) onGo, {
  TargetPlatform platform = TargetPlatform.android,
}) => MaterialApp(
  theme: claxTheme(Brightness.light, platform),
  home: Scaffold(
    body: Builder(
      builder: (context) => Center(
        child: TextButton(onPressed: () => onGo(context), child: const Text('Go')),
      ),
    ),
  ),
);

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

void main() {
  group('statusStyleFor', () {
    test('dark icons over white, light over black and mid grey', () {
      expect(statusStyleFor(Colors.white).statusBarIconBrightness, Brightness.dark);
      expect(statusStyleFor(Colors.black).statusBarIconBrightness, Brightness.light);
      expect(
        statusStyleFor(const Color(0xFF777777)).statusBarIconBrightness,
        Brightness.light,
      );
    });

    test('the bars are transparent and the gesture bar follows the icons', () {
      final s = statusStyleFor(Colors.black);
      expect(s.statusBarColor, Colors.transparent);
      expect(s.systemNavigationBarColor, Colors.transparent);
      expect(s.systemNavigationBarIconBrightness, Brightness.light);
      // iOS names the bar's brightness, not its icons'.
      expect(s.statusBarBrightness, Brightness.dark);
    });
  });

  group('ClaxScaffold', () {
    testWidgets('on Android it has an app bar and annotates the status bar', (
      tester,
    ) async {
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, TargetPlatform.android),
          home: const ClaxScaffold(
            title: 'Settings',
            subtitle: 'Clax',
            body: Text('body'),
          ),
        ),
      );
      expect(find.byType(AppBar), findsOneWidget);
      expect(find.text('Settings'), findsOneWidget);
      expect(find.text('Clax'), findsOneWidget);
      final regions = tester.widgetList<AnnotatedRegion<SystemUiOverlayStyle>>(
        find.byType(AnnotatedRegion<SystemUiOverlayStyle>),
      );
      expect(
        regions.map((r) => r.value.statusBarIconBrightness),
        everyElement(Brightness.dark),
      );
    });

    testWidgets('a large title collapses into the app bar', (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, TargetPlatform.android),
          home: ClaxScaffold(
            title: 'Inbox',
            largeTitle: true,
            body: ListView(children: [for (var i = 0; i < 40; i++) Text('row $i')]),
          ),
        ),
      );
      expect(find.byType(SliverAppBar), findsOneWidget);
      expect(find.text('row 0'), findsOneWidget);
    });

    testWidgets('on the Mac the window\'s toolbar carries the title', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      try {
        await tester.pumpWidget(
          MaterialApp(
            theme: claxTheme(Brightness.light, TargetPlatform.macOS),
            home: const ClaxScaffold(title: 'Settings', body: Text('body')),
          ),
        );
        expect(find.byType(AppBar), findsNothing);
        expect(find.text('Settings'), findsNothing);
        expect(find.text('body'), findsOneWidget);
      } finally {
        debugDefaultTargetPlatformOverride = null;
      }
    });
  });

  group('ClaxSheet', () {
    testWidgets('on a phone it is a bottom sheet with a handle and no close', (
      tester,
    ) async {
      window(tester, const Size(412, 915));
      await tester.pumpWidget(
        page(
          (c) => ClaxSheet.show<void>(
            c,
            title: 'Share',
            builder: (_) => const Text('content'),
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.byType(BottomSheet), findsOneWidget);
      expect(tester.widget<BottomSheet>(find.byType(BottomSheet)).showDragHandle, isTrue);
      expect(find.byIcon(Icons.close), findsNothing);
      expect(find.text('Share'), findsOneWidget);
      expect(find.text('content'), findsOneWidget);
    });

    testWidgets('wide it is a dialog at most 560 across', (tester) async {
      window(tester, const Size(900, 800));
      await tester.pumpWidget(
        page(
          (c) => ClaxSheet.show<void>(
            c,
            title: 'Share',
            builder: (_) => const SizedBox(width: 2000, child: Text('content')),
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.byType(BottomSheet), findsNothing);
      expect(find.byType(Dialog), findsOneWidget);
      final panel = find.descendant(
        of: find.byType(Dialog),
        matching: find.byType(Material),
      ).first;
      expect(tester.getSize(panel).width, lessThanOrEqualTo(560));
    });

    testWidgets('it answers what its content pops with', (tester) async {
      window(tester, const Size(412, 915));
      String? got;
      await tester.pumpWidget(
        page((c) async {
          got = await ClaxSheet.show<String>(
            c,
            builder: (s) => TextButton(
              onPressed: () => Navigator.pop(s, 'picked'),
              child: const Text('Pick'),
            ),
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Pick'));
      await tester.pumpAndSettle();
      expect(got, 'picked');
    });

    testWidgets('on a wide Mac it hangs from the top, 480 to 640 across, '
        'its actions bottom-right', (tester) async {
      window(tester, const Size(1200, 800));
      await tester.pumpWidget(
        page(
          platform: TargetPlatform.macOS,
          (c) => ClaxSheet.show<void>(
            c,
            title: 'Share',
            builder: (_) => const Text('content'),
            actions: [
              TextButton(onPressed: () {}, child: const Text('Cancel')),
              FilledButton(onPressed: () {}, child: const Text('Share')),
            ],
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.byType(BottomSheet), findsNothing);
      final sheet = tester.getRect(find.byKey(ClaxSheet.appleSheetKey));
      expect(sheet.top, 0);
      expect(sheet.width, inInclusiveRange(480, 640));
      expect(sheet.center.dx, closeTo(600, 1));
      final cancel = tester.getRect(find.text('Cancel'));
      final share = tester.getRect(find.widgetWithText(FilledButton, 'Share'));
      expect(cancel.right, lessThan(share.left));
      expect(share.right, closeTo(sheet.right - 20, 8));
    });
  });

  group('ClaxDialog', () {
    for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
      testWidgets('on $platform it answers the tapped index', (tester) async {
        window(tester, const Size(800, 800));
        int? got = -1;
        await tester.pumpWidget(
          page(platform: platform, (c) async {
            got = await ClaxDialog.show(
              c,
              title: 'Delete Groceries?',
              body: 'Its data goes too.',
              actions: const [
                ClaxDialogAction('Cancel'),
                ClaxDialogAction('Delete', kind: ClaxButtonKind.destructive),
              ],
              cancelIndex: 0,
            );
          }),
        );
        await tester.tap(find.text('Go'));
        await tester.pumpAndSettle();
        expect(find.text('Its data goes too.'), findsOneWidget);
        await tester.tap(find.text('Delete'));
        await tester.pumpAndSettle();
        expect(got, 1);
      });

      testWidgets('on $platform Escape answers the cancel index', (
        tester,
      ) async {
        window(tester, const Size(800, 800));
        int? got = -1;
        await tester.pumpWidget(
          page(platform: platform, (c) async {
            got = await ClaxDialog.show(
              c,
              title: 'Keep your changes?',
              actions: const [
                ClaxDialogAction('Discard', kind: ClaxButtonKind.destructive),
                ClaxDialogAction('Keep', kind: ClaxButtonKind.primary),
              ],
              defaultIndex: 1,
              cancelIndex: 0,
            );
          }),
        );
        await tester.tap(find.text('Go'));
        await tester.pumpAndSettle();
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        expect(got, 0);
      });

      testWidgets('on $platform Return answers the default index', (
        tester,
      ) async {
        window(tester, const Size(800, 800));
        int? got = -1;
        await tester.pumpWidget(
          page(platform: platform, (c) async {
            got = await ClaxDialog.show(
              c,
              title: 'Update Groceries?',
              actions: const [
                ClaxDialogAction('Keep mine'),
                ClaxDialogAction('Update', kind: ClaxButtonKind.primary),
              ],
              defaultIndex: 1,
              cancelIndex: 0,
            );
          }),
        );
        await tester.tap(find.text('Go'));
        await tester.pumpAndSettle();
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.pumpAndSettle();
        expect(got, 1);
      });
    }

    testWidgets('actions that do not fit stack full-width', (tester) async {
      window(tester, const Size(360, 800));
      await tester.pumpWidget(
        page(
          (c) => ClaxDialog.show(
            c,
            title: 'Share Groceries?',
            actions: const [
              ClaxDialogAction('Cancel'),
              ClaxDialogAction('Share without data'),
              ClaxDialogAction('Share a copy with my data', kind: ClaxButtonKind.primary),
            ],
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      final a = tester.getRect(find.ancestor(of: find.text('Cancel'), matching: find.byWidgetPredicate((w) => w is ButtonStyleButton)));
      final b = tester.getRect(find.ancestor(of: find.text('Share a copy with my data'), matching: find.byWidgetPredicate((w) => w is ButtonStyleButton)));
      expect(a.width, closeTo(b.width, 0.5));
      expect(a.left, closeTo(b.left, 0.5));
      // The default (rightmost when side by side) is on top.
      expect(b.top, lessThan(a.top));
    });

    testWidgets('actions that fit sit side by side on the right', (
      tester,
    ) async {
      window(tester, const Size(800, 800));
      await tester.pumpWidget(
        page(
          (c) => ClaxDialog.show(
            c,
            title: 'Delete Groceries?',
            actions: const [
              ClaxDialogAction('Cancel'),
              ClaxDialogAction('Delete', kind: ClaxButtonKind.destructive),
            ],
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      final a = tester.getRect(find.text('Cancel'));
      final b = tester.getRect(find.text('Delete'));
      expect(a.center.dy, closeTo(b.center.dy, 0.5));
      expect(a.right, lessThan(b.left));
    });
  });

  group('ClaxMenu', () {
    const items = [
      ClaxMenuItem('rename', 'Rename', icon: Icons.edit_outlined),
      ClaxMenuItem.separator(),
      ClaxMenuItem('delete', 'Delete', icon: Icons.delete_outline, destructive: true),
      ClaxMenuItem('off', 'Unavailable', enabled: false),
    ];

    /// Opens the menu at [anchor]; the list gets the menu's answer.
    Future<List<String?>> open(WidgetTester tester, Rect anchor) async {
      final result = <String?>[];
      await tester.pumpWidget(page((c) => ClaxMenu.show(c, items: items, anchor: anchor).then(result.add)));
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      return result;
    }

    testWidgets('it opens at its anchor and answers the tapped ID', (
      tester,
    ) async {
      window(tester, const Size(412, 915));
      const anchor = Rect.fromLTWH(40, 100, 48, 48);
      final result = await open(tester, anchor);
      final first = tester.getRect(find.text('Rename'));
      final item = tester.getRect(
        find.ancestor(of: find.text('Rename'), matching: find.byType(MenuItemButton)),
      );
      expect((item.top - anchor.bottom).abs(), lessThanOrEqualTo(8));
      expect((item.left - anchor.left).abs(), lessThanOrEqualTo(8));
      expect(first.top, greaterThan(anchor.top));
      await tester.tap(find.text('Delete'));
      await tester.pumpAndSettle();
      expect(result, ['delete']);
      expect(find.text('Rename'), findsNothing);
    });

    testWidgets('a tap outside answers null and leaves the page as it was', (
      tester,
    ) async {
      window(tester, const Size(412, 915));
      final result = await open(tester, const Rect.fromLTWH(40, 100, 48, 48));
      await tester.tapAt(const Offset(300, 800));
      await tester.pumpAndSettle();
      expect(result, [null]);
      expect(find.text('Rename'), findsNothing);
      expect(find.text('Go'), findsOneWidget);
    });

    testWidgets('a disabled item does nothing; Escape answers null', (
      tester,
    ) async {
      window(tester, const Size(412, 915));
      final result = await open(tester, const Rect.fromLTWH(40, 100, 48, 48));
      await tester.tap(find.text('Unavailable'), warnIfMissed: false);
      await tester.pumpAndSettle();
      expect(find.text('Rename'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(result, [null]);
    });
  });

  group('ClaxToast', () {
    testWidgets('at the top it sits under the status bar, 16 from the sides', (
      tester,
    ) async {
      window(tester, const Size(412, 915), top: 24);
      await tester.pumpWidget(
        page((c) => ClaxToast.show(c, 'Saved', place: ClaxToastPlace.top)),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      final card = tester.getRect(find.byKey(ClaxToast.cardKey));
      expect(card.top, greaterThanOrEqualTo(24));
      expect(card.left, 16);
      expect(card.right, 412 - 16);
      expect(find.text('Saved'), findsOneWidget);
      await tester.pump(const Duration(seconds: 4));
      await tester.pumpAndSettle();
      expect(find.byKey(ClaxToast.cardKey), findsNothing);
    });

    testWidgets('at the top over an app bar it still clears the status bar', (
      tester,
    ) async {
      window(tester, const Size(412, 915), top: 24);
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, TargetPlatform.android),
          home: ClaxScaffold(
            title: 'Groceries',
            body: Builder(
              builder: (c) => TextButton(
                onPressed: () =>
                    ClaxToast.show(c, 'Saved', place: ClaxToastPlace.top),
                child: const Text('Go'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(tester.getRect(find.byKey(ClaxToast.cardKey)).top, greaterThanOrEqualTo(24));
      await tester.pump(const Duration(seconds: 4));
      await tester.pumpAndSettle();
    });

    testWidgets('at the top on a wide window it is at most 560', (
      tester,
    ) async {
      window(tester, const Size(1280, 800), top: 24);
      await tester.pumpWidget(
        page((c) => ClaxToast.show(c, 'Saved', place: ClaxToastPlace.top)),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      final card = tester.getRect(find.byKey(ClaxToast.cardKey));
      expect(card.width, 560);
      expect(card.center.dx, 640);
      await tester.pump(const Duration(seconds: 4));
      await tester.pumpAndSettle();
      expect(find.byKey(ClaxToast.cardKey), findsNothing);
    });

    testWidgets('at the bottom it is a floating snack bar, 16 from the sides', (
      tester,
    ) async {
      window(tester, const Size(412, 915));
      var undone = false;
      await tester.pumpWidget(
        page(
          (c) => ClaxToast.show(
            c,
            'Deleted',
            actionLabel: 'Undo',
            onAction: () => undone = true,
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      final bar = tester.widget<SnackBar>(find.byType(SnackBar));
      expect(bar.behavior, SnackBarBehavior.floating);
      final rect = tester.getRect(
        find.descendant(of: find.byType(SnackBar), matching: find.byType(Material)).first,
      );
      expect(rect.left, closeTo(16, 0.5));
      expect(rect.right, closeTo(412 - 16, 0.5));
      await tester.tap(find.text('Undo'));
      await tester.pumpAndSettle();
      expect(undone, isTrue);
    });

    testWidgets('a plain snack bar on a phone keeps 16 from the sides', (
      tester,
    ) async {
      window(tester, const Size(412, 915));
      await tester.pumpWidget(
        page(
          (c) => ScaffoldMessenger.of(
            c,
          ).showSnackBar(const SnackBar(content: Text('Saved'))),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      final rect = tester.getRect(
        find.descendant(of: find.byType(SnackBar), matching: find.byType(Material)).first,
      );
      expect(rect.left, closeTo(16, 0.5));
      expect(rect.right, closeTo(412 - 16, 0.5));
    });

    testWidgets('inline it dismisses itself after 4 s on the clock', (
      tester,
    ) async {
      window(tester, const Size(1200, 800));
      final clock = _ManualClock();
      await tester.pumpWidget(
        page(
          (c) => ClaxToast.show(
            c,
            'Copied the link',
            place: ClaxToastPlace.inline,
            clock: clock,
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.text('Copied the link'), findsOneWidget);
      expect(tester.getRect(find.byKey(ClaxToast.cardKey)).top, 0);
      clock.elapse(const Duration(milliseconds: 3999));
      await tester.pumpAndSettle();
      expect(find.text('Copied the link'), findsOneWidget);
      clock.elapse(const Duration(milliseconds: 1));
      await tester.pumpAndSettle();
      expect(find.text('Copied the link'), findsNothing);
    });

    testWidgets('inline, by default, it goes after tester.pump(4 s)', (
      tester,
    ) async {
      window(tester, const Size(1200, 800));
      await tester.pumpWidget(
        page((c) => ClaxToast.show(c, 'Copied', place: ClaxToastPlace.inline)),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.text('Copied'), findsOneWidget);
      await tester.pump(const Duration(seconds: 4));
      await tester.pumpAndSettle();
      expect(find.text('Copied'), findsNothing);
    });

    testWidgets('inline it sits under the app bar', (tester) async {
      window(tester, const Size(412, 915), top: 24);
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, TargetPlatform.android),
          home: ClaxScaffold(
            title: 'Groceries',
            body: Builder(
              builder: (c) => TextButton(
                onPressed: () => ClaxToast.show(c, 'Copied', place: ClaxToastPlace.inline),
                child: const Text('Go'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      final bar = tester.getRect(find.byType(AppBar));
      final card = tester.getRect(find.byKey(ClaxToast.cardKey));
      expect(card.top, closeTo(bar.bottom, 0.5));
      expect(card.width, 412);
      await tester.pump(const Duration(seconds: 4));
      await tester.pumpAndSettle();
    });
  });
}
