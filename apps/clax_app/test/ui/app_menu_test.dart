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
          shortcuts: escapeShortcuts(),
          actions: escapeActions(() => nav.currentState?.maybePop()),
          home: const Scaffold(body: Text('apps')),
        ),
      ),
    );
    nav.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => const Scaffold(
          body: Column(children: [Text('pushed'), TextField()]),
        ),
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

  testWidgets('Esc in a text field never goes back and keeps its text; '
      'out of the field it does', (tester) async {
    await pumpPushed(tester);
    await tester.enterText(find.byType(TextField), 'a habit tracker');
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('pushed'), findsOneWidget);
    expect(find.text('a habit tracker'), findsOneWidget);
    FocusManager.instance.primaryFocus?.unfocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('pushed'), findsNothing);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('escapeOrBack unfocuses a focused field instead of going back', (
    tester,
  ) async {
    await pumpPushed(tester);
    await tester.showKeyboard(find.byType(TextField));
    var backs = 0;
    escapeOrBack(() => backs++);
    expect(backs, 0);
    await tester.pump();
    escapeOrBack(() => backs++);
    expect(backs, 1);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
