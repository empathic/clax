import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/split/sidebar.dart';
import 'package:clax_app/ui/split/sidebar_model.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';
import '../support/split_fixture.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

/// The test's mouse, once [_cursorAt] has put it down.
TestGesture? _mouse;

/// The mouse moved to [at] (put down there first), and the cursor the
/// window shows there.
Future<MouseCursor?> _cursorAt(WidgetTester tester, Offset at) async {
  if (_mouse case final mouse?) {
    await mouse.moveTo(at);
  } else {
    final mouse = _mouse = await tester.createGesture(
      kind: PointerDeviceKind.mouse,
      pointer: 1,
    );
    await mouse.addPointer(location: at);
    addTearDown(() async {
      await mouse.removePointer();
      _mouse = null;
    });
  }
  await tester.pump();
  return RendererBinding.instance.mouseTracker.debugDeviceActiveCursor(1);
}

/// [child] on the Mac's theme in a 1200 by 800 window.
Future<void> _pumpMac(WidgetTester tester, Widget child) async {
  tester.view.physicalSize = const Size(1200, 800);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MaterialApp(
      theme: claxTheme(Brightness.light, TargetPlatform.macOS),
      home: Scaffold(body: child),
    ),
  );
  await tester.pump();
}

void main() {
  group('on the Mac', () {
    testWidgets('a push button and a list row show the arrow, a link the '
        'pointing hand', (
      tester,
    ) async {
      await _pumpMac(
        tester,
        Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ClaxButton(label: 'Open', onPressed: () {}),
            ClaxButton(
              label: 'Make',
              kind: ClaxButtonKind.primary,
              isDefault: true,
              onPressed: () {},
            ),
            ClaxLink(label: 'console.anthropic.com', onPressed: () {}),
            ListTile(title: const Text('Licences'), onTap: () {}),
          ],
        ),
      );
      expect(
        await _cursorAt(tester, tester.getCenter(find.text('Licences'))),
        SystemMouseCursors.basic,
      );
      expect(
        await _cursorAt(tester, tester.getCenter(find.text('Open'))),
        SystemMouseCursors.basic,
      );
      expect(
        await _cursorAt(tester, tester.getCenter(find.text('Make'))),
        SystemMouseCursors.basic,
      );
      expect(
        await _cursorAt(
          tester,
          tester.getCenter(find.text('console.anthropic.com')),
        ),
        SystemMouseCursors.click,
      );
    }, variant: _mac);

    testWidgets('a link is underlined while hovered and runs on a click', (
      tester,
    ) async {
      var opened = 0;
      await _pumpMac(
        tester,
        ClaxLink(label: 'Licences', onPressed: () => opened++),
      );
      TextDecoration? decoration() => tester
          .widget<Text>(find.text('Licences'))
          .style
          ?.decoration;
      expect(decoration(), isNot(TextDecoration.underline));
      await _cursorAt(tester, tester.getCenter(find.text('Licences')));
      await tester.pump();
      expect(decoration(), TextDecoration.underline);
      await tester.tap(find.text('Licences'));
      expect(opened, 1);
      expect(
        tester.getSemantics(find.byType(ClaxLink)),
        isSemantics(label: 'Licences', isLink: true, hasTapAction: true),
      );
    }, variant: _mac);

    testWidgets('a sidebar row is filled with hover under the mouse, and '
        'shows the arrow', (tester) async {
      final f = SplitFixture();
      await tester.runAsync(f.start);
      final model = SidebarModel(host: f.host);
      addTearDown(() {
        model.dispose();
        f.dispose();
      });
      for (var i = 0; i < 5; i++) {
        await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      }
      await _pumpMac(
        tester,
        SizedBox(width: 220, child: ClaxSidebar(model: model)),
      );
      final row = find.byKey(ValueKey('sidebar-app-${f.ids['Groceries']}'));
      Color? fill() => (tester
              .widget<Container>(
                find.descendant(
                  of: row,
                  matching: find.byKey(const ValueKey('sidebar-row-fill')),
                ),
              )
              .decoration as BoxDecoration?)
          ?.color;
      expect(fill(), isNull);
      expect(
        await _cursorAt(tester, tester.getCenter(row)),
        SystemMouseCursors.basic,
      );
      await tester.pump();
      expect(fill(), ClaxTokens.light.hover);
    }, variant: _mac);

    testWidgets('a launcher cell is filled with hover and its icon lifts '
        'under the mouse, with the arrow', (tester) async {
      final api = FakeClaxApi(clock: () => '2026-10-12T00:00:00Z');
      api.seedArtifact('Groceries', _page);
      await _pumpMac(
        tester,
        Launcher(
          api: api,
          open: (_) {},
          make: (r, cancelled, stage) async => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
        ),
      );
      await tester.pumpAndSettle();
      final cell = find.ancestor(
        of: find.text('Groceries'),
        matching: find.byType(InkWell),
      );
      expect(tester.widget<InkWell>(cell).hoverColor, ClaxTokens.light.hover);
      double scale() => tester
          .widget<AnimatedScale>(
            find.descendant(of: cell, matching: find.byType(AnimatedScale)),
          )
          .scale;
      expect(scale(), 1);
      expect(
        await _cursorAt(tester, tester.getCenter(cell)),
        SystemMouseCursors.basic,
      );
      await tester.pumpAndSettle();
      expect(scale(), 1.03);
      final ink = Material.of(tester.element(cell)) as RenderObject;
      expect(ink, paints..rrect(color: ClaxTokens.light.hover));
    }, variant: _mac);

    testWidgets('an inbox row shows the arrow and fills with hover', (
      tester,
    ) async {
      await _pumpMac(
        tester,
        InboxRow(
          item: const InboxItem(
            id: 'i1',
            seq: 1,
            kind: 'version',
            read: false,
            createdAt: '2026-10-12T09:00:00Z',
            url: '/a/x',
            artifactId: 'x',
            artifactTitle: 'Run log',
            versionN: 4,
          ),
          now: DateTime.utc(2026, 10, 12, 10),
          onTap: () {},
        ),
      );
      final row = find.ancestor(
        of: find.text('Run log'),
        matching: find.byType(InkWell),
      );
      expect(tester.widget<InkWell>(row).hoverColor, ClaxTokens.light.hover);
      expect(
        await _cursorAt(tester, tester.getCenter(row)),
        SystemMouseCursors.basic,
      );
    }, variant: _mac);
  });

  testWidgets('on Android too a button shows the arrow and a link the '
      'pointing hand', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, TargetPlatform.android),
        home: Scaffold(
          body: Column(
            children: [
              ClaxButton(label: 'Open', onPressed: () {}),
              ClaxLink(label: 'Licences', onPressed: () {}),
            ],
          ),
        ),
      ),
    );
    expect(
      await _cursorAt(tester, tester.getCenter(find.text('Open'))),
      SystemMouseCursors.basic,
    );
    expect(
      await _cursorAt(tester, tester.getCenter(find.text('Licences'))),
      SystemMouseCursors.click,
    );
  });

  group('the system accent', () {
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    const light = Color(0xFFFF2D55);
    const dark = Color(0xFFFF375F);

    setUp(() {
      MacWindow.resetAccent();
      messenger.setMockMethodCallHandler(MacWindow.channel, (c) async {
        if (c.method == 'accentColor') {
          return {'light': '#ff2d55', 'dark': '#ff375f'};
        }
        return null;
      });
    });

    tearDown(() {
      messenger.setMockMethodCallHandler(MacWindow.channel, null);
      MacWindow.resetAccent();
    });

    Future<void> pump(WidgetTester tester, Widget child, Brightness b) async {
      await tester.pumpWidget(
        SystemAccentScope(
          notifier: MacWindow.accent,
          child: MaterialApp(
            // A new app each time, so the theme is not animated across.
            key: UniqueKey(),
            theme: claxTheme(b, TargetPlatform.macOS),
            home: Scaffold(body: Center(child: child)),
          ),
        ),
      );
      await tester.pump();
    }

    testWidgets('is the Mac\'s, as the window channel reports it, in the '
        'appearance shown', (tester) async {
      late BuildContext at;
      final probe = Builder(
        builder: (context) {
          at = context;
          return const SizedBox();
        },
      );
      await pump(tester, probe, Brightness.light);
      expect(appleAccent(at), light);
      await pump(tester, probe, Brightness.dark);
      expect(appleAccent(at), dark);
      // A change the Mac posts reaches what is built.
      await messenger.handlePlatformMessage(
        MacWindow.channel.name,
        MacWindow.channel.codec.encodeMethodCall(
          const MethodCall('accentColor', {
            'light': '#007aff',
            'dark': '#0a84ff',
          }),
        ),
        (_) {},
      );
      await tester.pump();
      expect(appleAccent(at), const Color(0xFF0A84FF));
    }, variant: _mac);

    testWidgets('without one it is the theme\'s primary', (tester) async {
      late BuildContext at;
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, TargetPlatform.macOS),
          home: Builder(
            builder: (context) {
              at = context;
              return const SizedBox();
            },
          ),
        ),
      );
      expect(appleAccent(at), Theme.of(at).colorScheme.primary);
    }, variant: _mac);

    testWidgets('rings a focused push button', (tester) async {
      final focus = FocusNode();
      addTearDown(focus.dispose);
      await pump(
        tester,
        ClaxButton(label: 'Open', onPressed: () {}),
        Brightness.light,
      );
      FocusManager.instance.highlightStrategy =
          FocusHighlightStrategy.alwaysTraditional;
      addTearDown(
        () => FocusManager.instance.highlightStrategy =
            FocusHighlightStrategy.automatic,
      );
      Focus.of(tester.element(find.text('Open'))).requestFocus();
      await tester.pump();
      final style = tester.widget<TextButton>(find.byType(TextButton)).style!;
      expect(
        style.side!.resolve({WidgetState.focused})!.color,
        light,
      );
    }, variant: _mac);

    testWidgets('fills the selected sidebar row while the sidebar has the '
        'keyboard', (tester) async {
      final f = SplitFixture();
      await tester.runAsync(f.start);
      final model = SidebarModel(host: f.host);
      addTearDown(() {
        model.dispose();
        f.dispose();
      });
      for (var i = 0; i < 5; i++) {
        await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      }
      await pump(
        tester,
        SizedBox(width: 220, height: 600, child: ClaxSidebar(model: model)),
        Brightness.light,
      );
      final row = find.byKey(ValueKey('sidebar-app-${f.ids['Groceries']}'));
      await tester.tap(row);
      await tester.pump();
      final fill = tester.widget<Container>(
        find.descendant(
          of: row,
          matching: find.byKey(const ValueKey('sidebar-row-fill')),
        ),
      );
      expect((fill.decoration! as BoxDecoration).color, light);
    }, variant: _mac);
  });
}
