import 'package:clax_app/ui/app_menu.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late List<MethodCall> menuCalls;

  setUp(() {
    menuCalls = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.menu, (call) async {
          menuCalls.add(call);
          return null;
        });
  });

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.menu, null);
  });

  /// A launcher with a pushed screen over it, under the menu bar.
  Future<CommentsCommand> pumpPushed(WidgetTester tester) async {
    final nav = GlobalKey<NavigatorState>();
    final comments = CommentsCommand();
    addTearDown(comments.dispose);
    await tester.pumpWidget(
      ClaxMenuBar(
        comments: comments,
        back: () => nav.currentState?.maybePop(),
        child: MaterialApp(
          navigatorKey: nav,
          home: const Scaffold(body: Text('apps')),
        ),
      ),
    );
    nav.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => const Scaffold(body: Text('pushed')),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('pushed'), findsOneWidget);
    return comments;
  }

  PlatformMenuItem item(WidgetTester tester, String label) {
    final bar = tester.widget<PlatformMenuBar>(find.byType(PlatformMenuBar));
    Iterable<PlatformMenuItem> all(List<PlatformMenuItem> items) =>
        items.expand(
          (i) => switch (i) {
            PlatformMenu(:final menus) => all(menus),
            PlatformMenuItemGroup(:final members) => all(members),
            _ => [i],
          },
        );
    return all(bar.menus).singleWhere((i) => i.label == label);
  }

  testWidgets('Esc goes back from a pushed screen, and does nothing on the '
      'apps', (tester) async {
    await pumpPushed(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('pushed'), findsNothing);
    expect(find.text('apps'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('apps'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('App › Back to Apps is ⌘[ and goes back', (tester) async {
    await pumpPushed(tester);
    final back = item(tester, 'Back to Apps');
    expect(back.shortcut, ClaxMenuBar.backShortcut);
    expect(
      ClaxMenuBar.backShortcut,
      const SingleActivator(LogicalKeyboardKey.bracketLeft, meta: true),
    );
    back.onSelected!();
    await tester.pumpAndSettle();
    expect(find.text('pushed'), findsNothing);
    expect(find.text('apps'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('Esc closes a dialog first', (tester) async {
    await pumpPushed(tester);
    final context = tester.element(find.text('pushed'));
    showDialog<void>(
      context: context,
      builder: (_) => const AlertDialog(content: Text('dialog')),
    );
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('dialog'), findsNothing);
    expect(find.text('pushed'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
