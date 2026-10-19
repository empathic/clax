import 'dart:ui'
    show DisplayFeature, DisplayFeatureState, DisplayFeatureType;

import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/kit/native_dialogs.dart';
import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/clax/api.dart' show StreamEvent;
import 'package:clax_app/ui/split/list_pane.dart';
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
final _android = TargetPlatformVariant.only(TargetPlatform.android);

void main() {
  late SplitFixture f;
  late SidebarModel model;
  late List<MethodCall> window;
  late List<MethodCall> toolbar;
  double? keptWidth;
  // Whether the window's toolbar has a search item (focusSearch's answer).
  var searchItem = false;
  final menus = <String>[];
  final deleted = <String>[];
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
    deleted.clear();
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
          detailFor: (s) => Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('detail of $s'),
                TextButton(onPressed: () {}, child: const Text('in detail')),
              ],
            ),
          ),
          onSidebarWidth: (w) => keptWidth = w,
          onMenu: (id, _) => menus.add(id),
          onDelete: (id) async => deleted.add(id),
          now: () => now,
          inbox: (_) => const Center(child: Text('inbox pane')),
          settings: (_) => const Center(child: Text('settings pane')),
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

  testWidgets('Command-Backspace asks before deleting the selected app, in '
      'the window\'s alert, and Delete deletes it', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    final alerts = <Map<Object?, Object?>>[];
    var answer = 1;
    final m = tester.binding.defaultBinaryMessenger;
    m.setMockMethodCallHandler(NativeDialogs.channel, (c) async {
      if (c.method != 'alert') return null;
      alerts.add(c.arguments as Map<Object?, Object?>);
      return {'index': answer, 'suppressed': false};
    });
    addTearDown(() => m.setMockMethodCallHandler(NativeDialogs.channel, null));
    Future<void> commandBackspace() async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pumpAndSettle();
    }

    await tester.tap(find.text('Groceries'));
    await tester.pump();
    await commandBackspace();
    expect(alerts, hasLength(1));
    expect(alerts.single['title'], 'Delete “Groceries”?');
    expect(alerts.single['buttons'], ['Delete', 'Cancel']);
    expect(alerts.single['destructive'], [0]);
    expect(alerts.single['defaultIndex'], 1);
    expect(alerts.single['cancelIndex'], 1);
    expect(deleted, isEmpty);

    answer = 0;
    await commandBackspace();
    expect(alerts, hasLength(2));
    expect(deleted, [f.ids['Groceries']]);
  }, variant: _mac);

  testWidgets('Command-Backspace asks nothing with Home selected or while '
      'a field has the keyboard', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    final alerts = <MethodCall>[];
    final m = tester.binding.defaultBinaryMessenger;
    m.setMockMethodCallHandler(NativeDialogs.channel, (c) async {
      alerts.add(c);
      return {'index': 0, 'suppressed': false};
    });
    addTearDown(() => m.setMockMethodCallHandler(NativeDialogs.channel, null));
    Future<void> commandBackspace() async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pumpAndSettle();
    }

    await commandBackspace();
    expect(alerts, isEmpty);

    await tester.tap(find.text('Groceries'));
    await tester.pump();
    await tester.enterText(find.byKey(const ValueKey('sidebar-filter')), 'gro');
    await commandBackspace();
    expect(alerts, isEmpty);
    expect(deleted, isEmpty);
  }, variant: _mac);

  testWidgets('Command-1 to Command-9 select the starred apps in order', (
    tester,
  ) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    Future<void> command(LogicalKeyboardKey key) async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(key);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pump();
    }

    await tester.tap(find.text('Home'));
    await command(LogicalKeyboardKey.digit2);
    expect(model.selection, AppSelected(f.ids['Notes']!));
    await command(LogicalKeyboardKey.digit1);
    expect(model.selection, AppSelected(f.ids['Runs']!));
    // Two are starred: a third selects nothing new.
    await command(LogicalKeyboardKey.digit3);
    expect(model.selection, AppSelected(f.ids['Runs']!));
  }, variant: _mac);

  testWidgets('Space in the sidebar opens the selected app: the keyboard '
      'goes into its detail', (tester) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    await tester.tap(find.text('Groceries'));
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.pump();
    final focused = FocusManager.instance.primaryFocus!.context!;
    expect(
      find.ancestor(
        of: find.byElementPredicate((e) => e == focused),
        matching: find.widgetWithText(TextButton, 'in detail'),
      ),
      findsOneWidget,
    );
  }, variant: _mac);

  testWidgets('Space on a make in the sidebar shows its build view', (
    tester,
  ) async {
    await pumpShell(
      tester,
      size: const Size(1200, 800),
    );
    await tester.tap(find.text('A tide table'));
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.pump();
    final id = (model.selection as MakeSelected).id;
    expect(f.actions.calls.last.$1, 'openMake');
    expect(f.actions.calls.last.$2, [id]);
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

  testWidgets('an iPad\'s split leaves the home app its own bar', (
    tester,
  ) async {
    await pumpShell(tester, size: const Size(1280, 800));
    expect(find.byType(SplitShell), findsOneWidget);
    expect(f.host.chrome.toolbar, isFalse);
  }, variant: _ipad);

  /// The rail's destination labelled [label].
  Finder railItem(String label) => find.descendant(
    of: find.byKey(const ValueKey('split-rail')),
    matching: find.text(label),
  );

  testWidgets('on Android at 1280 by 800 a rail of 80, a list pane of 360 '
      'and the detail beside them', (tester) async {
    await pumpShell(tester, size: const Size(1280, 800));
    expect(tester.getSize(find.byKey(const ValueKey('split-rail'))).width, 80);
    expect(find.byType(ClaxListPane), findsOneWidget);
    expect(find.byType(ClaxSidebar), findsNothing);
    expect(
      tester.getSize(find.byKey(const ValueKey('split-sidebar'))).width,
      360,
    );
    expect(
      tester.getTopLeft(find.byKey(const ValueKey('split-sidebar'))).dx,
      80,
    );
    // The rail, the list and a hairline, then the detail to the edge.
    final detail = find.byKey(const ValueKey('split-detail'));
    expect(tester.getTopLeft(detail).dx, 441);
    expect(tester.getTopRight(detail).dx, 1280);
    expect(find.text('home grid'), findsOneWidget);
    // Home is the rail's; the list has Starred, Apps and Making.
    for (final label in ['New', 'Home', 'Inbox', 'Settings']) {
      expect(railItem(label), findsOneWidget, reason: label);
    }
    final pane = find.byType(ClaxListPane);
    for (final title in ['Starred', 'Apps', 'Making', 'Tools', 'Groceries']) {
      expect(
        find.descendant(of: pane, matching: find.text(title)),
        findsOneWidget,
        reason: title,
      );
    }
    expect(find.descendant(of: pane, matching: find.text('Home')), findsNothing);
    await tester.tap(find.descendant(of: pane, matching: find.text('Groceries')));
    await tester.pump();
    expect(
      find.text('detail of AppSelected(${f.ids['Groceries']})'),
      findsOneWidget,
    );
  }, variant: _android);

  testWidgets('on a foldable at 1280 the split between the list and the '
      'detail sits on a vertical hinge', (tester) async {
    tester.view.displayFeatures = [
      const DisplayFeature(
        bounds: Rect.fromLTWH(640, 0, 0, 800),
        type: DisplayFeatureType.hinge,
        state: DisplayFeatureState.postureFlat,
      ),
    ];
    addTearDown(tester.view.resetDisplayFeatures);
    await pumpShell(tester, size: const Size(1280, 800));
    final detail = find.byKey(const ValueKey('split-detail'));
    expect(tester.getTopLeft(detail).dx, 640);
    expect(tester.getTopRight(detail).dx, 1280);
    expect(
      tester.getTopRight(find.byKey(const ValueKey('split-sidebar'))).dx,
      639,
      reason: 'the hairline ends on the hinge',
    );

    // A hinge with a width of its own: the list ends at its left, the
    // detail starts at its right.
    tester.view.displayFeatures = [
      const DisplayFeature(
        bounds: Rect.fromLTWH(630, 0, 20, 800),
        type: DisplayFeatureType.hinge,
        state: DisplayFeatureState.postureFlat,
      ),
    ];
    await tester.pump();
    expect(tester.getTopLeft(detail).dx, 650);
    expect(
      tester.getTopRight(find.byKey(const ValueKey('split-sidebar'))).dx,
      629,
    );
  }, variant: _android);

  testWidgets('a horizontal fold, a cutout or a hinge too near an edge leaves '
      'the list pane 360', (tester) async {
    await pumpShell(tester, size: const Size(1280, 800));
    final detail = find.byKey(const ValueKey('split-detail'));
    for (final feature in const [
      DisplayFeature(
        bounds: Rect.fromLTWH(0, 400, 1280, 0),
        type: DisplayFeatureType.fold,
        state: DisplayFeatureState.postureHalfOpened,
      ),
      DisplayFeature(
        bounds: Rect.fromLTWH(600, 0, 80, 30),
        type: DisplayFeatureType.cutout,
        state: DisplayFeatureState.unknown,
      ),
      DisplayFeature(
        bounds: Rect.fromLTWH(200, 0, 0, 800),
        type: DisplayFeatureType.hinge,
        state: DisplayFeatureState.postureFlat,
      ),
      DisplayFeature(
        bounds: Rect.fromLTWH(1100, 0, 0, 800),
        type: DisplayFeatureType.hinge,
        state: DisplayFeatureState.postureFlat,
      ),
    ]) {
      tester.view.displayFeatures = [feature];
      await tester.pump();
      expect(tester.getTopLeft(detail).dx, 441, reason: '$feature');
    }
    tester.view.resetDisplayFeatures();
  }, variant: _android);

  testWidgets('on Android at 700 the rail and one pane: the grid, and an app '
      'in its place until Home', (tester) async {
    await pumpShell(tester, size: const Size(700, 900));
    expect(tester.getSize(find.byKey(const ValueKey('split-rail'))).width, 80);
    expect(find.byType(ClaxListPane), findsNothing);
    expect(find.text('home grid'), findsOneWidget);
    expect(tester.getTopLeft(find.byKey(const ValueKey('split-detail'))).dx, 80);
    model.select(AppSelected(f.ids['Budget']!));
    await tester.pump();
    expect(find.text('detail of AppSelected(${f.ids['Budget']})'), findsOneWidget);
    await tester.tap(railItem('Home'));
    await tester.pump();
    expect(find.text('home grid'), findsOneWidget);
    expect(model.selection, const HomeSelected());
  }, variant: _android);

  testWidgets('the rail\'s Inbox shows the unread count, and no badge at 0', (
    tester,
  ) async {
    await pumpShell(tester, size: const Size(1280, 800));
    Finder count(String n) => find.descendant(
      of: find.byKey(const ValueKey('split-rail')),
      matching: find.text(n),
    );
    Future<void> unread(int n) async {
      f.api.emitStream(
        StreamEvent(
          topic: 'inbox',
          type: 'inbox_read',
          data: {'topic': 'inbox', 'ids': null, 'read': true, 'unread': n},
        ),
      );
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pump();
      expect(f.host.unread, n);
    }

    await unread(3);
    expect(count('3'), findsOneWidget);
    await unread(0);
    expect(count('3'), findsNothing);
    expect(count('0'), findsNothing);
  }, variant: _android);

  testWidgets('New app on the rail opens the New app sheet', (tester) async {
    await pumpShell(tester, size: const Size(1280, 800));
    await tester.tap(find.byKey(const ValueKey('rail-new')));
    await tester.pump();
    expect(f.actions.calls.last.$1, 'newApp');
    f.actions.calls.clear();
    await tester.tap(railItem('New'));
    await tester.pump();
    expect(f.actions.calls.single.$1, 'newApp');
  }, variant: _android);

  testWidgets('Inbox and Settings are rail destinations in place of the '
      'panes; Home, Back or an app chosen elsewhere brings the panes back', (
    tester,
  ) async {
    await pumpShell(tester, size: const Size(1280, 800));
    final shell = tester.state<SplitShellState>(find.byType(SplitShell));
    await tester.tap(railItem('Inbox'));
    await tester.pump();
    expect(find.text('inbox pane'), findsOneWidget);
    expect(find.byType(ClaxListPane), findsNothing);
    expect(shell.place, SplitPlace.inbox);
    await tester.tap(railItem('Settings'));
    await tester.pump();
    expect(find.text('settings pane'), findsOneWidget);
    expect(find.text('inbox pane'), findsNothing);
    await tester.tap(railItem('Home'));
    await tester.pump();
    expect(find.text('home grid'), findsOneWidget);
    expect(find.byType(ClaxListPane), findsOneWidget);

    // The platform's Back from a destination goes Home.
    expect(shell.go(SplitPlace.inbox), isTrue);
    await tester.pump();
    expect(find.text('inbox pane'), findsOneWidget);
    await tester.binding.handlePopRoute();
    await tester.pump();
    expect(shell.place, SplitPlace.home);
    expect(find.text('home grid'), findsOneWidget);

    // An app opened from the inbox shows in the panes.
    shell.go(SplitPlace.inbox);
    await tester.pump();
    model.select(AppSelected(f.ids['Weather']!));
    await tester.pump();
    expect(shell.place, SplitPlace.home);
    expect(find.text('detail of AppSelected(${f.ids['Weather']})'), findsOneWidget);
  }, variant: _android);

  testWidgets('the home app\'s grid stays built under Inbox and Settings', (
    tester,
  ) async {
    await pumpShell(tester, size: const Size(1280, 800));
    final shell = tester.state<SplitShellState>(find.byType(SplitShell));
    final grid = tester.element(find.text('home grid'));
    shell.go(SplitPlace.settings);
    await tester.pump();
    expect(find.text('home grid'), findsNothing);
    shell.go(SplitPlace.home);
    await tester.pump();
    expect(tester.element(find.text('home grid')), same(grid));
  }, variant: _android);

  testWidgets('without a rail no destination is taken: the Mac', (
    tester,
  ) async {
    await pumpShell(tester, size: const Size(1200, 800));
    final shell = tester.state<SplitShellState>(find.byType(SplitShell));
    expect(find.byKey(const ValueKey('split-rail')), findsNothing);
    expect(shell.go(SplitPlace.inbox), isFalse);
    expect(shell.place, SplitPlace.home);
  }, variant: _mac);

  testWidgets('the list pane\'s search narrows the list and the grid; '
      'Control-F puts the keyboard in it', (tester) async {
    await pumpShell(tester, size: const Size(1280, 800));
    final search = find.byKey(const ValueKey('list-pane-search'));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();
    await tester.pump();
    final field = find.descendant(of: search, matching: find.byType(EditableText));
    expect(tester.widget<EditableText>(field).focusNode.hasFocus, isTrue);
    await tester.enterText(search, 'gro');
    await tester.pump();
    expect(model.filter, 'gro');
    expect(f.host.query, 'gro');
    final pane = find.byType(ClaxListPane);
    expect(find.descendant(of: pane, matching: find.text('Groceries')), findsOneWidget);
    expect(find.descendant(of: pane, matching: find.text('Budget')), findsNothing);
  }, variant: _android);

  testWidgets('wide Android holds the toolbar for the home app while the list '
      'pane carries Search and the rail New app, Inbox and Settings; at 700, '
      'with no Search beside the grid, the home app keeps its bar', (
    tester,
  ) async {
    await pumpShell(tester, size: const Size(1280, 800));
    expect(f.host.chrome.toolbar, isTrue);
    expect(f.host.chrome.form, 'wide');
    tester.view.physicalSize = const Size(700, 900);
    await tester.pump();
    await tester.pump();
    expect(f.host.chrome.toolbar, isFalse);
    tester.view.physicalSize = const Size(1280, 800);
    await tester.pump();
    await tester.pump();
    expect(f.host.chrome.toolbar, isTrue);
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
    expect(f.host.chrome.toolbar, isFalse);
  }, variant: _android);

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
    expect(await fits(const Size(599, 900), TargetPlatform.android), isFalse);
    expect(await fits(const Size(700, 900), TargetPlatform.android), isTrue);
    expect(await fits(const Size(1280, 800), TargetPlatform.android), isTrue);
    expect(await fits(const Size(700, 900), TargetPlatform.macOS), isTrue);
    expect(await fits(const Size(700, 900), TargetPlatform.iOS), isTrue);
    expect(await fits(const Size(402, 874), TargetPlatform.iOS), isFalse);
    expect(ClaxFamily.forPlatform(TargetPlatform.iOS), ClaxFamily.apple);
  });
}
