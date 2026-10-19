import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/ui/app_menu.dart';
import 'package:clax_app/ui/shortcuts.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// Home actions that record what ran.
class _Actions implements HomeActions {
  final ran = <String>[];
  @override
  void importApp() => ran.add('importApp');
  @override
  void newApp() => ran.add('newApp');
  @override
  void openInbox() => ran.add('openInbox');
  @override
  void openSettings() => ran.add('openSettings');
  @override
  void open(String id) => ran.add('open $id');
  @override
  void openMake(String id) => ran.add('openMake $id');
  @override
  Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items) =>
      Future.value();
}

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

void main() {
  late List<MethodCall> menuCalls;
  late List<MethodCall> flutterMenuCalls;

  setUp(() {
    menuCalls = [];
    flutterMenuCalls = [];
    final m = TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    m.setMockMethodCallHandler(MacMenu.channel, (call) async {
      menuCalls.add(call);
      return null;
    });
    m.setMockMethodCallHandler(SystemChannels.menu, (call) async {
      flutterMenuCalls.add(call);
      return null;
    });
  });

  tearDown(() {
    final m = TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    m.setMockMethodCallHandler(MacMenu.channel, null);
    m.setMockMethodCallHandler(SystemChannels.menu, null);
  });

  /// [command] chosen in the native menu bar.
  Future<void> choose(WidgetTester tester, ClaxCommand command) async {
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      MacMenu.channel.name,
      MacMenu.channel.codec.encodeMethodCall(
        MethodCall('command', command.tag),
      ),
      (_) {},
    );
    await tester.pumpAndSettle();
  }

  /// The enabled state last sent for [c], or null when none was sent.
  bool? enabled(ClaxCommand c) {
    bool? on;
    for (final call in menuCalls) {
      final args = call.arguments as Map<Object?, Object?>;
      if (call.method == 'setEnabled' && args['tag'] == c.tag) {
        on = args['enabled'] as bool;
      }
    }
    return on;
  }

  /// A launcher with a pushed screen over it, under the menu bar.
  Future<
    ({
      CommentsCommand comments,
      GlobalKey<NavigatorState> nav,
      _Actions actions,
    })
  >
  pumpPushed(WidgetTester tester, {VoidCallback? settings}) async {
    final nav = GlobalKey<NavigatorState>();
    final comments = CommentsCommand();
    addTearDown(comments.dispose);
    final actions = _Actions();
    await tester.pumpWidget(
      ClaxMenuBar(
        comments: comments,
        navigator: () => nav.currentState,
        home: actions,
        settings: settings,
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
    return (comments: comments, nav: nav, actions: actions);
  }

  testWidgets('on the Mac no PlatformMenuBar is built: the menu bar is '
      'MainMenu.xib, whose items the bar enables', (tester) async {
    var settings = 0;
    final s = await pumpPushed(tester, settings: () => settings++);
    expect(find.byType(PlatformMenuBar), findsNothing);
    expect(flutterMenuCalls, isEmpty);
    expect(enabled(ClaxCommand.settings), isTrue);
    expect(enabled(ClaxCommand.newApp), isTrue);
    expect(enabled(ClaxCommand.importApp), isTrue);
    expect(enabled(ClaxCommand.inbox), isTrue);
    expect(enabled(ClaxCommand.back), isTrue);
    expect(enabled(ClaxCommand.home), isTrue);
    // Nothing runs them yet.
    expect(enabled(ClaxCommand.reload), isFalse);
    expect(enabled(ClaxCommand.star), isFalse);
    // Comment and Threads follow the open app screen.
    expect(enabled(ClaxCommand.comment), isFalse);
    expect(enabled(ClaxCommand.toggleThreads), isFalse);
    var toggles = 0;
    void toggle() => toggles++;
    s.comments.register(toggle, comment: () {});
    await tester.pump();
    expect(enabled(ClaxCommand.toggleThreads), isTrue);
    expect(enabled(ClaxCommand.comment), isTrue);
    await choose(tester, ClaxCommand.toggleThreads);
    expect(toggles, 1);
    s.comments.unregister(toggle);
    await tester.pump();
    expect(enabled(ClaxCommand.toggleThreads), isFalse);
    await choose(tester, ClaxCommand.toggleThreads);
    expect(toggles, 1);

    await choose(tester, ClaxCommand.settings);
    expect(settings, 1);
  }, variant: _mac);

  testWidgets('a state already sent is not sent again', (tester) async {
    final s = await pumpPushed(tester);
    final before = menuCalls.length;
    s.comments.register(() {});
    await tester.pump();
    final sent = menuCalls.sublist(before);
    expect(sent.map((c) => (c.arguments as Map)['tag']).toSet(), {
      ClaxCommand.toggleThreads.tag,
    });
  }, variant: _mac);

  testWidgets('off the Mac nothing is sent and no menu bar is built', (
    tester,
  ) async {
    await pumpPushed(tester);
    expect(find.byType(PlatformMenuBar), findsNothing);
    expect(menuCalls, isEmpty);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets('App › Home goes to the grid from two screens deep', (
    tester,
  ) async {
    final s = await pumpPushed(tester);
    s.nav.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => const Scaffold(body: Text('deeper')),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('deeper'), findsOneWidget);
    await choose(tester, ClaxCommand.home);
    expect(find.text('deeper'), findsNothing);
    expect(find.text('pushed'), findsNothing);
    expect(find.text('apps'), findsOneWidget);
  }, variant: _mac);

  testWidgets('File › New App…, Import App… and Window › Inbox run the home '
      'screen’s flows', (tester) async {
    final s = await pumpPushed(tester);
    await choose(tester, ClaxCommand.importApp);
    await choose(tester, ClaxCommand.newApp);
    await choose(tester, ClaxCommand.inbox);
    expect(s.actions.ran, ['importApp', 'newApp', 'openInbox']);
  }, variant: _mac);

  testWidgets('without the home screen’s flows New App…, Import App… and '
      'Inbox are disabled', (tester) async {
    await tester.pumpWidget(
      ClaxMenuBar(
        comments: CommentsCommand(),
        child: const MaterialApp(home: Text('apps')),
      ),
    );
    await tester.pump();
    expect(enabled(ClaxCommand.newApp), isFalse);
    expect(enabled(ClaxCommand.importApp), isFalse);
    expect(enabled(ClaxCommand.inbox), isFalse);
    expect(enabled(ClaxCommand.settings), isFalse);
  }, variant: _mac);

  testWidgets('App › Back steps the page’s history (GoBack) before leaving '
      'the screen', (tester) async {
    final s = await pumpPushed(tester);
    // A screen whose page has one step of history, as the app screen's
    // PopScope holds back while its page can go back.
    final goBacks = <String>[];
    var history = 1;
    s.nav.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => StatefulBuilder(
          builder: (context, setState) => PopScope(
            canPop: history == 0,
            onPopInvokedWithResult: (didPop, _) {
              if (didPop) return;
              goBacks.add('GoBack');
              setState(() => history--);
            },
            child: const Scaffold(body: Text('app')),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await choose(tester, ClaxCommand.back);
    expect(goBacks, ['GoBack']);
    expect(find.text('app'), findsOneWidget);
    await choose(tester, ClaxCommand.back);
    expect(goBacks, ['GoBack']);
    expect(find.text('app'), findsNothing);
    expect(find.text('pushed'), findsOneWidget);
  }, variant: _mac);

  testWidgets('a ClaxCommandIntent runs the same command as the menu', (
    tester,
  ) async {
    final s = await pumpPushed(tester);
    Actions.invoke(
      tester.element(find.text('pushed')),
      const ClaxCommandIntent(ClaxCommand.importApp),
    );
    expect(s.actions.ran, ['importApp']);
  }, variant: _mac);

  group('Edit over dev.empathic.clax/edit', () {
    late List<MethodCall> platform;
    String? clipboard;

    setUp(() {
      platform = [];
      clipboard = null;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, (call) async {
            platform.add(call);
            switch (call.method) {
              case 'Clipboard.setData':
                clipboard = (call.arguments as Map)['text'] as String?;
              case 'Clipboard.getData':
                return clipboard == null ? null : {'text': clipboard};
              case 'Clipboard.hasStrings':
                return {'value': clipboard != null};
            }
            return null;
          });
    });
    tearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );

    /// [action] sent by EditBridge.swift; answers whether a field took it.
    Future<Object?> edit(WidgetTester tester, String action) async {
      Object? reply;
      await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
        MacEdit.channel.name,
        MacEdit.channel.codec.encodeMethodCall(MethodCall(action)),
        (data) => reply = data == null
            ? null
            : MacEdit.channel.codec.decodeEnvelope(data),
      );
      await tester.pumpAndSettle();
      return reply;
    }

    testWidgets('select all, copy, cut, paste, undo and redo reach the '
        'focused Flutter field', (tester) async {
      await pumpPushed(tester);
      await tester.enterText(find.byType(TextField), 'groceries');
      await tester.pump();
      final field = tester.widget<TextField>(find.byType(TextField));
      String text() =>
          tester.widget<EditableText>(find.byType(EditableText)).controller.text;

      expect(await edit(tester, 'selectAll'), isTrue);
      expect(await edit(tester, 'copy'), isTrue);
      expect(clipboard, 'groceries');
      expect(await edit(tester, 'cut'), isTrue);
      expect(text(), '');
      expect(await edit(tester, 'paste'), isTrue);
      expect(text(), 'groceries');
      expect(await edit(tester, 'pasteAsPlainText'), isTrue);
      expect(text(), 'groceriesgroceries');
      expect(field.controller, isNull, reason: 'the field owns its text');
      expect(await edit(tester, 'undo'), isTrue);
      expect(await edit(tester, 'redo'), isTrue);
    }, variant: _mac);

    testWidgets('with nothing focused the action is not taken', (
      tester,
    ) async {
      await pumpPushed(tester);
      FocusManager.instance.primaryFocus?.unfocus();
      await tester.pump();
      expect(await edit(tester, 'copy'), isFalse);
      expect(await edit(tester, 'bogus'), isFalse);
      expect(platform.where((c) => c.method == 'Clipboard.setData'), isEmpty);
    }, variant: _mac);
  });

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
  }, variant: _mac);

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
  }, variant: _mac);

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
  }, variant: _mac);

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
  }, variant: _mac);
}
