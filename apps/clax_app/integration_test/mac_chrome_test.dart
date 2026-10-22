// The Mac app's window chrome, menus and real interactions, in the built
// debug app on the embedded core: the native toolbar, title bar and
// sidebar material, the Settings and Inbox windows, the menu bar, the
// alert and the sheets, each captured in light and dark through the
// Runner's test channel (`macos/Runner/TestChannel.swift`), and a
// person's clicks, drags and keys sent as AppKit events.
//
// Run: `bazelisk test //apps/clax_app:integration_mac_chrome`; the PNGs go
// to `$CLAX_MAC_SHOTS_DIR` (pass
// `--test_env=CLAX_MAC_SHOTS_DIR=$PWD/target/flutter-app/mac`), else to the
// test's outputs. Keys reach only an active app: on a locked screen, or
// with another app in front, the key checks are skipped and say so.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/ui/ai_consent_sheet.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/ask_prompt.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

import 'rig.dart';

/// Where the PNGs go.
final _shots = () {
  final env = Platform.environment;
  final dir =
      env['CLAX_MAC_SHOTS_DIR'] ??
      '${env['TEST_UNDECLARED_OUTPUTS_DIR'] ?? Directory.systemTemp.path}/mac';
  Directory(dir).createSync(recursive: true);
  return dir;
}();

/// The committed menu golden, as the test target gives it
/// (`CLAX_MENU_GOLDEN`, relative to the runfiles directory it starts in).
final _golden = Platform.environment['CLAX_MENU_GOLDEN'];

const _themes = {'light': 'aqua', 'dark': 'darkAqua'};

/// The app's page: a heading to pick, a field to copy from and paste into,
/// and room to draw an area.
const _page = '''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Chrome check</title>
<style>
:root { --bg: #ffffff; --fg: #1d1d1f; }
@media (prefers-color-scheme: dark) { :root { --bg: #1d1d1f; --fg: #f5f5f7; } }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.4 system-ui, sans-serif; }
h1 { margin: 0; height: 96px; line-height: 96px; padding: 0 16px; }
input { position: absolute; left: 16px; top: 112px; width: 240px; height: 32px; font: inherit; }
#area { position: absolute; left: 16px; right: 16px; top: 180px; height: 360px; }
</style>
</head>
<body>
<h1 id="t">Chrome check</h1>
<input id="f" value="page words">
<div id="area"></div>
</body>
</html>
''';

/// A make that waits until the test ends, so its build view stays up.
class _HeldMake implements AgentBackend {
  final started = Completer<void>();
  final _release = Completer<void>();

  void release() {
    if (!_release.isCompleted) _release.complete();
  }

  @override
  String get name => 'scripted';

  @override
  Future<EditResult> run(AgentRequest request) async {
    if (!started.isCompleted) started.complete();
    await _release.future;
    throw StateError('the test ended');
  }
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the Mac window chrome, menus and real input', (tester) async {
    final held = _HeldMake();
    final rig = await Rig.create(backend: held, homeApp: true);
    addTearDown(rig.close);
    addTearDown(held.release);
    final notes = <String>[];
    void note(String s) {
      notes.add(s);
      debugPrint('mac_chrome: $s');
    }

    addTearDown(() => debugPrint('mac_chrome notes:\n${notes.join('\n')}'));
    // The person's pasteboard, put back after.
    final board = await MacChrome.pasteboard();
    addTearDown(() => MacChrome.pasteboard(board ?? ''));
    addTearDown(() => MacChrome.setAppearance(null));

    // --- An app to select, made before the app starts.
    await rig.api.registerAppSession(
      installId: 'mac-chrome-seed',
      pid: pid,
      cwd: rig.dir.path,
    );
    final made = await rig.api.createArtifact(
      PublishRequest(title: 'Chrome check', files: {'index.html': _page}),
    );
    final aid = made.artifact.id;

    await runClaxApp(rig.services);
    final home = await rig.until<Finder>(
      tester,
      'the sidebar',
      () => find.byKey(const ValueKey('sidebar-home')),
    );
    expect(home, findsOneWidget);
    final appRow = await rig.until<Finder>(
      tester,
      'the app in the sidebar',
      () => find.byKey(ValueKey('sidebar-app-$aid')),
    );
    final w = await MacChrome.window();
    note('main window: $w');
    final active = w['active'] == true;
    // A click on what Flutter draws: through AppKit while the app is
    // active; otherwise (AppKit gives an inactive app's views no clicks)
    // the tap handler of the GestureDetector under it, as Flutter calls it.
    Future<void> tapFlutter(Finder f) async {
      if (active) {
        final at = tester.getCenter(f);
        await MacChrome.post([
          for (final t in ['move', 'down', 'up'])
            {'type': t, 'x': at.dx, 'y': at.dy},
        ]);
      } else {
        final g = tester.widget<GestureDetector>(
          find
              .descendant(
                of: f,
                matching: find.byWidgetPredicate(
                  (w) => w is GestureDetector && w.onTap != null,
                ),
              )
              .first,
        );
        g.onTap!();
      }
      await tester.pump();
    }

    // An [animated] state (the build view's live progress) never holds
    // still: it is captured once its appearance is in.
    Future<void> shoot(
      String state, {
      String? window,
      bool animated = false,
    }) async {
      for (final MapEntry(key: theme, value: look) in _themes.entries) {
        await MacChrome.setAppearance(look);
        await rig.until(
          tester,
          'the $theme appearance',
          () =>
              tester.platformDispatcher.platformBrightness ==
              (theme == 'dark' ? Brightness.dark : Brightness.light),
        );
        if (animated) {
          for (var i = 0; i < 5; i++) {
            await tester.pump(const Duration(milliseconds: 16));
          }
          expect(
            await MacChrome.captureWindow(
              '$_shots/$state-$theme.png',
              window: window,
            ),
            isNotNull,
          );
        } else {
          await MacChrome.captureSettled(
            tester,
            '$_shots/$state-$theme.png',
            window: window,
          );
        }
      }
      await MacChrome.setAppearance(null);
    }

    // --- Home selected, under the toolbar's search and Refresh, once the
    // home app is installed and shown.
    final homeApp = await _within(
      tester,
      rig,
      () =>
          find.byKey(const ValueKey('home-fallback')).evaluate().isEmpty &&
          find
              .byWidgetPredicate(
                (w) =>
                    w.key is ValueKey<String> &&
                    (w.key! as ValueKey<String>).value.startsWith('home-app-'),
              )
              .evaluate()
              .isNotEmpty,
      wait: const Duration(seconds: 45),
    );
    note('the home app shows at Home: $homeApp');
    await shoot('home');
    final homeBar = await MacChrome.toolbar();
    note('home toolbar: $homeBar');
    note(
      'home toolbar search fields: '
      '${homeBar.where((c) => c['kind'] == 'search').length}',
    );

    // --- The menu bar: its items and keys against the golden, each menu
    // open.
    final menu = await MacChrome.describeMenu();
    var menusBlank = false;
    File('$_shots/main_menu.txt').writeAsStringSync(menu);
    for (final title in [
      'Clax',
      'File',
      'Edit',
      'View',
      'App',
      'Window',
      'Help',
    ]) {
      for (final MapEntry(key: theme, value: look) in _themes.entries) {
        await MacChrome.setAppearance(look);
        await tester.pump();
        final size = await MacChrome.openMenu(
          title,
          '$_shots/menu-${title.toLowerCase()}-$theme.png',
        );
        expect(size, isNotNull, reason: 'the $title menu never showed');
        if (size!['blank'] == true) menusBlank = true;
      }
    }
    await MacChrome.setAppearance(null);
    note(
      menusBlank
          ? 'menus: the display composites nothing (locked or asleep), so the '
                'menu PNGs are blank; run on an unlocked Mac to see them'
          : 'menus: captured from the screen',
    );

    // --- An app selected, by a real click on its row.
    await tapFlutter(appRow);
    await rig.until(
      tester,
      'the app\'s first ready',
      () => rig.messages.whereType<Ready>().any((r) => r.artifact == aid),
    );
    expect(find.byType(AppScreen), findsOneWidget);
    await shoot('app');
    final appBar = await MacChrome.toolbar();
    note('app toolbar: $appBar');
    Map<String, Object?> item(List<Map<String, Object?>> bar, String label) =>
        bar.firstWhere(
          (c) => '${c['label']}'.split('|').contains(label),
          orElse: () => fail('no toolbar item "$label" in $bar'),
        );
    for (final label in ['Back', 'Forward', 'Comment', 'Threads']) {
      item(appBar, label);
    }
    final sidebar = tester.getRect(
      find.byKey(const ValueKey('split-sidebar')).first,
    );
    final edge = appBar.where((c) => c['kind'] == 'sidebarEdge').firstOrNull;
    note('sidebar edge spacer: $edge; the sidebar: $sidebar');
    expect(edge, isNotNull, reason: 'no spacer at the sidebar\'s edge');
    final ids = [for (final c in appBar) c['id']];
    expect(
      ids.indexOf('clax.sidebarEdge'),
      lessThan(ids.indexOf('group:navigation')),
      reason: 'Back and Forward come after the sidebar\'s edge: $ids',
    );
    if (edge!.containsKey('x')) {
      // Where AppKit lays the spacer out: only a window a display shows
      // has its toolbar laid out for real.
      note(
        'the sidebar edge spacer ends at '
        '${(edge['x']! as num) + (edge['width']! as num)} '
        '(the sidebar ends at ${sidebar.right}); window on a display: '
        '${!(w['locked'] == true)}',
      );
    }

    // --- The sidebar's context menu, from a real right click.
    for (final MapEntry(key: theme, value: look) in _themes.entries) {
      await MacChrome.setAppearance(look);
      await tester.pump();
      await MacChrome.armMenuCapture('$_shots/sidebar-menu-$theme.png');
      if (active) {
        final at = tester.getCenter(appRow);
        await MacChrome.post([
          {'type': 'move', 'x': at.dx, 'y': at.dy},
          {'type': 'rightDown', 'x': at.dx, 'y': at.dy},
          {'type': 'rightUp', 'x': at.dx, 'y': at.dy},
        ]);
      } else {
        // What the row's secondary click calls.
        final g = tester.widget<GestureDetector>(
          find
              .descendant(
                of: appRow,
                matching: find.byWidgetPredicate(
                  (w) => w is GestureDetector && w.onSecondaryTapUp != null,
                ),
              )
              .first,
        );
        g.onSecondaryTapUp!(
          TapUpDetails(
            kind: PointerDeviceKind.mouse,
            globalPosition: tester.getCenter(appRow),
          ),
        );
      }
      Map<String, Object?>? shot;
      final shown = await _within(tester, rig, () async {
        shot = await MacChrome.menuCaptured();
        return shot != null;
      });
      note('the sidebar\'s context menu opens ($theme): $shown');
      expect(shown, isTrue, reason: 'the app row\'s menu never opened');
      if (shot?['blank'] == true) menusBlank = true;
    }
    await MacChrome.setAppearance(null);

    // --- Comment mode, from a real click on the toolbar's Comment.
    final page = find.descendant(
      of: find.byType(AppScreen),
      matching: find.byType(WebViewWidget),
    );
    expect(await MacChrome.pressToolbarItem('Comment'), isTrue);
    await rig.untilState(tester, 'comment mode', (s) => s.commenting);
    final web = tester.getRect(page);
    final frame = await _frameRect(rig);
    Offset inPage(double x, double y) =>
        web.topLeft + frame.topLeft + Offset(x, y);

    // A real pick: the pointer arrives on the heading and clicks it, with
    // nothing attested by the app.
    final heading = inPage(60, 48);
    await MacChrome.post([
      for (final t in ['move', 'down', 'up'])
        {'type': t, 'x': heading.dx, 'y': heading.dy},
    ]);
    final picked = await _within(
      tester,
      rig,
      () => rig.messages.whereType<StateMsg>().lastOrNull?.draft != null,
    );
    note('a real click on the heading picks it: $picked (app active: $active)');
    if (active) {
      expect(picked, isTrue, reason: 'a real click in the page picked nothing');
    }
    if (!picked) {
      // The app's attestation, as the integration rig's comment does.
      await rig.surface!.call(
        AttestPointer(
          pointer: 1,
          phase: PointerPhase.down,
          x: heading.dx - web.left,
          y: heading.dy - web.top,
          devicePixelRatio: tester.view.devicePixelRatio,
        ),
      );
      await MacChrome.post([
        for (final t in ['down', 'up'])
          {'type': t, 'x': heading.dx, 'y': heading.dy},
      ]);
      await rig.untilState(tester, 'the attested pick', (s) => s.draft != null);
    }
    await rig.until(
      tester,
      'the composer popover',
      () => find.byKey(const ValueKey('composer-popover')),
    );
    await shoot('comment');

    // Escape with the page focused closes the composer.
    final escaped = await _key(tester, rig, '\u{1b}', 53, () {
      final s = rig.messages.whereType<StateMsg>().lastOrNull;
      return s?.draft == null;
    });
    note('Escape in the page closes the composer: $escaped');
    if (active) {
      expect(escaped, isTrue, reason: 'Escape in the page left the composer up');
    } else if (!escaped) {
      await rig.surface!.call(const CancelDraft());
    }
    await rig.untilState(tester, 'the composer closed', (s) => s.draft == null);
    if (!rig.messages.whereType<StateMsg>().last.commenting) {
      await rig.surface!.call(const EnterCommentMode());
      await rig.untilState(tester, 'comment mode again', (s) => s.commenting);
    }

    // An area drawn in the page and let go over the sidebar.
    final from = inPage(40, 220);
    final to = inPage(200, 380);
    final outside = Offset(sidebar.center.dx, to.dy);
    final how = await MacChrome.post([
      {'type': 'down', 'x': from.dx, 'y': from.dy},
      for (var i = 1; i <= 6; i++)
        {
          'type': 'drag',
          'x': from.dx + (to.dx - from.dx) * i / 6,
          'y': from.dy + (to.dy - from.dy) * i / 6,
        },
      {'type': 'drag', 'x': outside.dx, 'y': outside.dy},
      {'type': 'up', 'x': outside.dx, 'y': outside.dy},
    ]);
    final area = await _within(
      tester,
      rig,
      () => rig.messages.whereType<StateMsg>().lastOrNull?.draft?.kind == 'area',
    );
    await MacChrome.captureSettled(tester, '$_shots/area-release-light.png');
    note('an area let go over the sidebar is picked: $area (events $how)');
    if (how == 'queued') {
      expect(
        area,
        isTrue,
        reason: 'the release over the sidebar never reached the page',
      );
    }
    if (area) {
      await rig.surface!.call(const CancelDraft());
      await rig.untilState(tester, 'the area closed', (s) => s.draft == null);
    }
    await rig.surface!.call(const ExitCommentMode());
    await rig.untilState(tester, 'comment mode off', (s) => !s.commenting);

    // --- The threads pane, from the toolbar's Threads.
    expect(await MacChrome.pressToolbarItem('Threads'), isTrue);
    await rig.until(
      tester,
      'the threads pane',
      () => find.byKey(const ValueKey('threads-pane-edge')),
    );
    await shoot('threads');
    expect(await MacChrome.pressToolbarItem('Threads'), isTrue);
    await rig.until(
      tester,
      'the threads pane closed',
      () => find.byKey(const ValueKey('threads-pane-edge')).evaluate().isEmpty,
    );

    // --- Copy and paste, in the page's field and in a Flutter field: keys
    // reach only an active app's key window.
    if (active) {
      final field = inPage(120, 128);
      await MacChrome.click(field);
      await _key(tester, rig, 'a', 0, () => true, modifiers: const ['cmd']);
      final pageCopy = await _key(
        tester,
        rig,
        'c',
        8,
        () async => await MacChrome.pasteboard() == 'page words',
        modifiers: const ['cmd'],
      );
      expect(pageCopy, isTrue, reason: '⌘A ⌘C in the page\'s field');
      await MacChrome.pasteboard('pasted words');
      await _key(tester, rig, 'v', 9, () => true, modifiers: const ['cmd']);
      await _key(tester, rig, 'a', 0, () => true, modifiers: const ['cmd']);
      await MacChrome.pasteboard('');
      final pasted = await _key(
        tester,
        rig,
        'c',
        8,
        () async => await MacChrome.pasteboard() == 'pasted words',
        modifiers: const ['cmd'],
      );
      expect(pasted, isTrue, reason: '⌘V in the page\'s field');
      final filter = find.descendant(
        of: find.byKey(const ValueKey('sidebar-filter')),
        matching: find.byType(EditableText),
      );
      await MacChrome.click(tester.getCenter(filter));
      await tester.enterText(filter, 'Chrome');
      await tester.pump();
      await _key(tester, rig, 'a', 0, () => true, modifiers: const ['cmd']);
      final flutterCopy = await _key(
        tester,
        rig,
        'c',
        8,
        () async => await MacChrome.pasteboard() == 'Chrome',
        modifiers: const ['cmd'],
      );
      expect(flutterCopy, isTrue, reason: '⌘A ⌘C in the sidebar\'s filter');
      await MacChrome.pasteboard('check');
      final flutterPaste = await _key(
        tester,
        rig,
        'v',
        9,
        () => tester.widget<EditableText>(filter).controller.text == 'check',
        modifiers: const ['cmd'],
      );
      expect(flutterPaste, isTrue, reason: '⌘V in the sidebar\'s filter');
      await tester.enterText(filter, '');
      await tester.pump();
    } else {
      note('⌘C and ⌘V: not tried, the app is not active (${w['locked'] == true ? 'the screen is locked' : 'another app is in front'})');
    }

    // --- The permission alert, before and after it arms.
    final ctx = tester.element(find.byType(AppScreen));
    final stop = Completer<void>();
    final waiting = AskPrompt.show(
      ctx,
      const AskMsg(
        id: 'chrome-1',
        kind: AskKind.capability,
        title: 'Allow “Chrome check” to reach the internet?',
        body: 'It asks to load example.com.',
        allow: 'Allow',
        deny: 'Don’t allow',
        always: 'Always for this app',
        armMs: 600000,
      ),
      cancelled: stop.future,
    );
    await rig.until(
      tester,
      'the alert sheet',
      () async => (await MacChrome.window())['sheet'] == true,
    );
    await shoot('alert-waiting');
    stop.complete();
    expect(await waiting, AskAnswerValue.dismiss);
    await rig.until(
      tester,
      'the alert gone',
      () async => (await MacChrome.window())['sheet'] == false,
    );
    final stopArmed = Completer<void>();
    final armed = AskPrompt.show(
      ctx,
      const AskMsg(
        id: 'chrome-2',
        kind: AskKind.capability,
        title: 'Allow “Chrome check” to reach the internet?',
        body: 'It asks to load example.com.',
        allow: 'Allow',
        deny: 'Don’t allow',
        armMs: 1,
      ),
      cancelled: stopArmed.future,
    );
    await rig.until(
      tester,
      'the armed alert sheet',
      () async => (await MacChrome.window())['sheet'] == true,
    );
    await shoot('alert-armed');
    if (active) {
      // A held Return does nothing; a fresh one presses Allow.
      await MacChrome.key('\r', 36, isRepeat: true);
      final answered = await _key(tester, rig, '\r', 36, () async {
        return (await MacChrome.window())['sheet'] == false;
      });
      expect(answered, isTrue, reason: 'Return left the armed alert up');
      expect(await armed, AskAnswerValue.allow);
    } else {
      note('the armed alert\'s keys: not tried, the app is not active');
      stopArmed.complete();
      expect(await armed, AskAnswerValue.dismiss);
    }
    await rig.until(
      tester,
      'the armed alert gone',
      () async => (await MacChrome.window())['sheet'] == false,
    );

    // --- A sheet, sliding from under the toolbar.
    final consent = showAiConsentSheet(ctx);
    await rig.until(
      tester,
      'the consent sheet',
      () => find.text('Not now'),
    );
    await shoot('sheet');
    await tester.tap(find.text('Not now'));
    expect(await consent, isNot(true));

    // --- Settings, each tab, from the Clax menu.
    expect(await MacChrome.chooseMenuItem('Clax', 'Settings…'), isTrue);
    await rig.until(
      tester,
      'the Settings window',
      () async => (await MacChrome.window(window: 'settings'))['open'] == true,
    );
    final tabs = await rig.until<List<String>>(
      tester,
      'the Settings window\'s tabs',
      () async {
        final found = [
          for (final c in await MacChrome.toolbar(window: 'settings'))
            if (c['kind'] == 'button') '${c['label']}',
        ];
        return found.isEmpty ? null : found;
      },
    );
    note('Settings tabs: $tabs');
    expect(tabs, isNotEmpty, reason: 'the Settings window has no tabs');
    for (final tab in tabs) {
      expect(
        await MacChrome.pressToolbarItem(tab, window: 'settings'),
        isTrue,
      );
      await rig.until(
        tester,
        'the $tab tab selected',
        () async => (await MacChrome.toolbar(
          window: 'settings',
        )).any((c) => c['label'] == tab && c['on'] == true),
      );
      await shoot(
        'settings-${tab.toLowerCase().replaceAll(' ', '-')}',
        window: 'settings',
      );
    }

    // --- The Inbox window, from the Window menu.
    expect(await MacChrome.chooseMenuItem('Window', 'Inbox'), isTrue);
    await rig.until(
      tester,
      'the Inbox window',
      () async => (await MacChrome.window(window: 'inbox'))['open'] == true,
    );
    final inboxBar = await rig.until<List<Map<String, Object?>>>(
      tester,
      'the Inbox window\'s toolbar',
      () async {
        final bar = await MacChrome.toolbar(window: 'inbox');
        return bar.any((c) => c['kind'] == 'group') ? bar : null;
      },
    );
    note('Inbox toolbar: $inboxBar');
    expect(
      inboxBar.where((c) => c['kind'] == 'search'),
      hasLength(1),
      reason: 'the Inbox window\'s search field',
    );
    await shoot('inbox', window: 'inbox');

    // --- The system accent: asked, and pushed again when it changes.
    final accent = MacWindow.accent;
    note('the system accent: ${accent.value}');
    MacWindow.resetAccent();
    await MacChrome.colorsChanged();
    await rig.until(
      tester,
      'the accent pushed',
      () => MacWindow.accent.value != null,
    );

    // --- A make in the detail, beside the toolbar's Back and Forward.
    expect(await MacChrome.chooseMenuItem('File', 'New App…'), isTrue);
    final ask = await rig.until<Finder>(
      tester,
      'the New sheet',
      () => find.text('Make it'),
    );
    await shoot('new-sheet');
    await tester.enterText(find.byType(TextField).last, 'A tally counter');
    await tester.pump();
    await tester.tap(ask);
    await rig.until(tester, 'the make started', () => held.started.isCompleted);
    final makeRow = await rig.until<Finder>(
      tester,
      'the make in the sidebar',
      () => find.byWidgetPredicate(
        (w) =>
            w.key is ValueKey<String> &&
            (w.key! as ValueKey<String>).value.startsWith('sidebar-make-'),
      ),
    );
    await tapFlutter(makeRow.first);
    final back = await rig.until<Finder>(
      tester,
      'the build view',
      () => find.byKey(const ValueKey('build-back')),
    );
    await shoot('make', animated: true);
    final makeBar = await MacChrome.toolbar();
    final chevron = tester.getRect(back.first);
    note('make toolbar: $makeBar; the build view\'s Back: $chevron');

    // --- The menu bar's items and keys, against the golden.
    if (_golden case final g?) {
      expect(
        menu,
        File(g).readAsStringSync(),
        reason:
            'the main menu differs from integration_test/goldens/main_menu.txt; '
            'the menu as found is in $_shots/main_menu.txt',
      );
    } else {
      note('no CLAX_MENU_GOLDEN: the menu was written, not compared');
    }
  });
}

/// The content frame's box in the WebView, in CSS pixels.
Future<Rect> _frameRect(Rig rig) async {
  final raw = await rig.surface!.evaluate(
    'JSON.stringify(document.querySelector("iframe.frame").getBoundingClientRect())',
  );
  final j = (jsonDecode(raw is String ? raw : '$raw') as Map)
      .cast<String, Object?>();
  double n(String k) => (j[k] as num).toDouble();
  return Rect.fromLTWH(n('left'), n('top'), n('width'), n('height'));
}

/// Pumps until [probe] holds, for at most [wait]; whether it did.
Future<bool> _within(
  WidgetTester tester,
  Rig rig,
  FutureOr<bool> Function() probe, {
  Duration wait = const Duration(seconds: 10),
}) async {
  final end = DateTime.now().add(wait);
  while (DateTime.now().isBefore(end)) {
    await tester.pump();
    if (await probe()) return true;
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }
  return false;
}

/// Sends a key and waits for [then]; whether it held.
Future<bool> _key(
  WidgetTester tester,
  Rig rig,
  String chars,
  int code,
  FutureOr<bool> Function() then, {
  List<String> modifiers = const [],
}) async {
  await MacChrome.key(chars, code, modifiers: modifiers);
  return _within(tester, rig, then);
}
