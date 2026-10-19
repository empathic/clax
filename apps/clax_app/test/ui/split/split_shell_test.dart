import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/split/sidebar.dart';
import 'package:clax_app/ui/split/sidebar_model.dart';
import 'package:clax_app/ui/split/split_shell.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/split_fixture.dart';

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);
final _ipad = TargetPlatformVariant.only(TargetPlatform.iOS);

void main() {
  late SplitFixture f;
  late SidebarModel model;
  late List<MethodCall> window;
  late List<MethodCall> toolbar;
  double? keptWidth;
  // Whether the window's toolbar has a search item (focusSearch's answer).
  var searchItem = false;
  final menus = <String>[];
  var now = DateTime.utc(2026, 10, 19, 9);

  /// The shell in a window of [size], on the platform the test's variant
  /// sets.
  Future<void> pumpShell(WidgetTester tester, {required Size size}) async {
    final platform = defaultTargetPlatform;
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    MacToolbar.reset();
    window = [];
    toolbar = [];
    keptWidth = null;
    menus.clear();
    final m = tester.binding.defaultBinaryMessenger;
    m.setMockMethodCallHandler(MacWindow.channel, (c) async {
      window.add(c);
      return null;
    });
    m.setMockMethodCallHandler(MacToolbar.channel, (c) async {
      toolbar.add(c);
      return c.method == 'focusSearch' ? searchItem : null;
    });
    addTearDown(() {
      m.setMockMethodCallHandler(MacWindow.channel, null);
      m.setMockMethodCallHandler(MacToolbar.channel, null);
    });
    f = SplitFixture();
    await tester.runAsync(f.start);
    model = SidebarModel(host: f.host);
    addTearDown(() {
      model.dispose();
      f.dispose();
    });
    for (var i = 0; i < 5; i++) {
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    }
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, platform),
        home: SplitShell(
          model: model,
          home: const Center(child: Text('home grid')),
          detailFor: (s) => Center(child: Text('detail of $s')),
          onSidebarWidth: (w) => keptWidth = w,
          onMenu: (id, _) => menus.add(id),
          now: () => now,
        ),
      ),
    );
    await tester.pump();
  }

  List<Map<Object?, Object?>> sent(List<MethodCall> calls, String method) => [
    for (final c in calls)
      if (c.method == method) c.arguments as Map<Object?, Object?>,
  ];

  testWidgets('a 1200 by 800 Mac window shows the sidebar 220 wide beside '
      'the home grid, over the window\'s sidebar material', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    expect(find.byType(ClaxSidebar), findsOneWidget);
    expect(tester.getSize(find.byKey(const ValueKey('split-sidebar'))).width, 220);
    expect(find.text('home grid'), findsOneWidget);
    expect(sent(window, 'setSidebarWidth').last['width'], 220);
    // Clear, for the material behind the Flutter view.
    final box = tester.widget<ColoredBox>(
      find
          .descendant(
            of: find.byType(ClaxSidebar),
            matching: find.byType(ColoredBox),
          )
          .first,
    );
    expect(box.color, Colors.transparent);
    // The toggle leads the toolbar once a screen declares its items.
    expect(sent(toolbar, 'setItems'), isEmpty);
    MacToolbar.setItems(
      Object(),
      const [ToolbarItemSpec('back', symbol: 'chevron.left', label: 'Back')],
      view: tester.view.viewId,
    );
    await tester.pump();
    final items = sent(toolbar, 'setItems').last['items'] as List;
    expect((items.first as Map)['id'], 'sidebar');
    expect((items.first as Map)['label'], 'Hide Sidebar');
  }, variant: _mac);

  testWidgets('selecting an app shows its detail and names the window after '
      'it; Home shows the grid again', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    await tester.tap(find.text('Groceries'));
    await tester.pump();
    expect(find.text('detail of AppSelected(${f.ids['Groceries']})'), findsOneWidget);
    // The grid is kept, offstage.
    expect(find.text('home grid', skipOffstage: false), findsOneWidget);
    expect(find.text('home grid'), findsNothing);
    expect(sent(window, 'setTitle').last['title'], 'Groceries');
    await tester.tap(find.text('Home'));
    await tester.pump();
    expect(find.text('home grid'), findsOneWidget);
    expect(sent(window, 'setTitle').last['title'], 'Clax');
  }, variant: _mac);

  testWidgets('the sidebar\'s edge resizes it within 180 to 320, and the width '
      'it ends at is kept', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    final edge = find.byKey(const ValueKey('split-divider'));
    await tester.drag(edge, const Offset(50, 0));
    await tester.pump();
    expect(tester.getSize(find.byKey(const ValueKey('split-sidebar'))).width, 270);
    expect(keptWidth, 270);
    await tester.drag(edge, const Offset(400, 0));
    await tester.pump();
    expect(keptWidth, 320);
  }, variant: _mac);

  testWidgets('the toolbar\'s toggle and Control-Command-S hide and show it', (
    tester,
  ) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    // With the keyboard in the sidebar.
    await tester.tap(find.text('Home'));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.control);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyS);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.control);
    await tester.pump();
    expect(find.byType(ClaxSidebar), findsNothing);
    await tester.pump();
    expect(sent(window, 'setSidebarWidth').last['width'], 0);
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      MacToolbar.channel.name,
      MacToolbar.channel.codec.encodeMethodCall(
        MethodCall('clicked', {'view': tester.view.viewId, 'id': 'sidebar'}),
      ),
      (_) {},
    );
    await tester.pump();
    expect(find.byType(ClaxSidebar), findsOneWidget);
  }, variant: _mac);

  testWidgets('Command-F puts the keyboard in the toolbar\'s search with Home '
      'selected, and in the sidebar\'s filter otherwise or without one', (
    tester,
  ) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    Future<void> commandF() async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.pump();
      await tester.pump();
    }

    bool filterFocused() => tester
        .widget<TextField>(find.byKey(const ValueKey('sidebar-filter')))
        .focusNode!
        .hasFocus;

    searchItem = true;
    await tester.tap(find.text('Home'));
    await commandF();
    expect(sent(toolbar, 'focusSearch'), [
      {'view': tester.view.viewId, 'id': 'search'},
    ]);
    expect(filterFocused(), isFalse);

    // A Home with no search item in the toolbar: the filter.
    searchItem = false;
    await commandF();
    expect(sent(toolbar, 'focusSearch'), hasLength(2));
    expect(filterFocused(), isTrue);

    // With an app selected, the filter, and no search is asked for.
    searchItem = true;
    await tester.tap(find.text('Groceries'));
    await tester.pump();
    await commandF();
    expect(sent(toolbar, 'focusSearch'), hasLength(2));
    expect(filterFocused(), isTrue);
  }, variant: _mac);

  testWidgets('at 700 on the Mac the sidebar is hidden until toggled, and '
      'goes once a row is chosen', (tester) async {
    await pumpShell(
      tester,
      size: const Size(700, 600),
    );
    expect(find.byType(ClaxSidebar), findsNothing);
    expect(find.text('home grid'), findsOneWidget);
    tester.state<SplitShellState>(find.byType(SplitShell)).toggleSidebar();
    await tester.pump();
    expect(find.byType(ClaxSidebar), findsOneWidget);
    // Over the detail: no material behind it.
    expect(sent(window, 'setSidebarWidth').last['width'], 0);
    await tester.tap(find.text('Budget'));
    await tester.pump();
    expect(find.byType(ClaxSidebar), findsNothing);
    expect(find.text('detail of AppSelected(${f.ids['Budget']})'), findsOneWidget);
  }, variant: _mac);

  testWidgets('arrows move the selection and typing selects by title', (
    tester,
  ) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    await tester.tap(find.text('Home'));
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(model.selection, AppSelected(f.ids['Runs']!));
    await tester.sendKeyEvent(LogicalKeyboardKey.keyG);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyR);
    await tester.pump();
    expect(model.selection, AppSelected(f.ids['Groceries']!));
    // A pause starts a new prefix.
    now = now.add(const Duration(seconds: 2));
    await tester.sendKeyEvent(LogicalKeyboardKey.keyW);
    await tester.pump();
    expect(model.selection, AppSelected(f.ids['Weather']!));
  }, variant: _mac);

  testWidgets('a group row opens, saying so; a secondary click on an app '
      'asks for its menu', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    final semantics = tester.ensureSemantics();
    expect(find.text('Hammer'), findsNothing);
    expect(
      tester.getSemantics(find.byKey(const ValueKey('sidebar-group-tools'))),
      matchesSemantics(
        label: 'Tools, 2 apps',
        isButton: true,
        hasExpandedState: true,
        isExpanded: false,
        hasTapAction: true,
      ),
    );
    await tester.tap(find.text('Tools'));
    await tester.pump();
    expect(find.text('Hammer'), findsOneWidget);
    await tester.tap(find.text('Saw'), buttons: kSecondaryButton);
    await tester.pump();
    expect(menus, [f.ids['Saw']]);
    semantics.dispose();
  }, variant: _mac);

  testWidgets('an app dragged onto Starred is starred there', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    final from = tester.getCenter(find.text('Budget'));
    final to = tester.getCenter(find.text('Runs'));
    final g = await tester.startGesture(from, kind: PointerDeviceKind.mouse);
    await g.moveTo(from + const Offset(0, -10));
    await tester.pump();
    await g.moveTo(to);
    await tester.pump();
    await g.up();
    for (var i = 0; i < 5; i++) {
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pump();
    }
    final doc = await tester.runAsync(
      () => f.api.getDoc(f.home, 'layout/main'),
    );
    expect(doc!.data['starred'], [f.ids['Budget'], f.ids['Runs'], f.ids['Notes']]);
  }, variant: _mac);

  testWidgets('on an iPad at 1180 the sidebar is the Mac\'s, with no Mac '
      'window chrome', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1180, 820),
    );
    expect(find.byType(ClaxSidebar), findsOneWidget);
    expect(tester.getSize(find.byKey(const ValueKey('split-sidebar'))).width, 220);
    expect(window, isEmpty);
    expect(toolbar, isEmpty);
  }, variant: _ipad);

  testWidgets('a Mac window\'s split holds the toolbar for the home app: '
      'chrome() says toolbar and wide while it shows, and not once it goes', (
    tester,
  ) async {
    await pumpShell(tester, size: const Size(1200, 800));
    expect(f.host.chrome.toolbar, isTrue);
    expect(f.host.chrome.form, 'wide');
    expect(
      await tester.runAsync(() => f.host.call(f.home, 'chrome', {})),
      containsPair('toolbar', true),
    );
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
    expect(f.host.chrome.toolbar, isFalse);
  }, variant: _mac);

  testWidgets('without Mac window chrome the split leaves the home app its '
      'own bar: an iPad, and Android at 1280', (tester) async {
    await pumpShell(tester, size: const Size(1280, 800));
    expect(find.byType(SplitShell), findsOneWidget);
    expect(f.host.chrome.toolbar, isFalse);
  }, variant: TargetPlatformVariant(
    const {TargetPlatform.iOS, TargetPlatform.android},
  ));

  testWidgets('the split fits by size class and family, never by platform', (
    tester,
  ) async {
    Future<bool> fits(Size size, TargetPlatform platform) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      late bool out;
      await tester.pumpWidget(
        MaterialApp(
          // A new app each time, so the theme is not animated across.
          key: UniqueKey(),
          theme: claxTheme(Brightness.light, platform),
          home: Builder(
            builder: (context) {
              out = SplitShell.fits(context);
              return const SizedBox();
            },
          ),
        ),
      );
      return out;
    }

    addTearDown(tester.view.reset);
    expect(await fits(const Size(412, 915), TargetPlatform.android), isFalse);
    expect(await fits(const Size(700, 900), TargetPlatform.android), isFalse);
    expect(await fits(const Size(1280, 800), TargetPlatform.android), isTrue);
    expect(await fits(const Size(700, 900), TargetPlatform.macOS), isTrue);
    expect(await fits(const Size(700, 900), TargetPlatform.iOS), isTrue);
    expect(await fits(const Size(402, 874), TargetPlatform.iOS), isFalse);
    expect(ClaxFamily.forPlatform(TargetPlatform.iOS), ClaxFamily.apple);
  });
}
