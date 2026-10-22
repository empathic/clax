// The Mac form of the main screens, under a window's toolbar row as the Mac
// app draws them (`ClaxViews`): nothing a person reads or clicks is in the
// window buttons' corner, and an open app leaves the row to the window's
// own toolbar, whose items it declares. With CLAX_SHOTS_DIR each screen is
// also written there as `mac-<screen>-<theme>.png`, light and dark, with
// the window buttons drawn in; the open app's declared toolbar goes beside
// it as `mac-app-toolbar.json`, for the native toolbar to be drawn over it:
//   SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
//   bazelisk test //apps/clax_app:ui_mac_traffic_lights_test \
//     --nocache_test_results --test_env=CLAX_SHOTS_DIR="$SHOTS" \
//     --sandbox_writable_path="$SHOTS"
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/build_plan.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_surface.dart';
import '../../support/fake_voice.dart';
import '../../support/paths.dart';
import '../build_view_test.dart' show FakeBuilds;
import '../shots/rig.dart' show loadFonts, shotsDir;

final _out = shotsDir;
final _shots = _out != null;

/// The main window's toolbar row as macOS 26 lays out its icon-only
/// unified toolbar (`Titlebar(of:)` in macos/Runner/WindowBridge.swift
/// measures it): 52 points high, the zoom button's right edge 79 points in.
const _bar = WindowTitlebar(top: 52, leading: 79);

final _onMac = TargetPlatformVariant.only(TargetPlatform.macOS);

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-test';
  @override
  Future<void> write(String? key) async {}
}

class _Timer implements Timer {
  @override
  bool get isActive => true;
  @override
  int get tick => 0;
  @override
  void cancel() {}
}

class _Preview implements PreviewView {
  @override
  Future<void> loadHtml(String html) async {}
  @override
  Widget build(BuildContext context) => const SizedBox.expand();
  @override
  void dispose() {}
}

const _item = TextStyle(fontSize: 17, color: Color(0xFF1D1D1F));

/// An app's page as the WebView would show it, a light page whatever the
/// theme: a header and a list.
class _Page extends FakeAppSurface {
  @override
  Widget build(BuildContext context) => const ColoredBox(
    key: ValueKey('page'),
    color: Color(0xFFFFFFFF),
    child: Padding(
      padding: EdgeInsets.all(32),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Groceries',
            style: TextStyle(
              fontSize: 34,
              fontWeight: FontWeight.w700,
              color: Color(0xFF1D1D1F),
            ),
          ),
          SizedBox(height: 16),
          Text('Oat milk', style: _item),
          SizedBox(height: 8),
          Text('Lemons', style: _item),
          SizedBox(height: 8),
          Text('Coffee beans', style: _item),
        ],
      ),
    ),
  );
}

/// Every text and every tap target on screen, by where it is drawn.
List<(String, Rect)> _readOrClick(WidgetTester tester) {
  final out = <(String, Rect)>[];
  void visit(Element e) {
    final w = e.widget;
    final ro = e.renderObject;
    final box = ro is RenderBox && ro.hasSize && ro.attached ? ro : null;
    final tappable =
        w is RawGestureDetector &&
        w.gestures.keys.any(
          (t) =>
              t == TapGestureRecognizer ||
              t == DoubleTapGestureRecognizer ||
              t == LongPressGestureRecognizer,
        );
    final text = ro is RenderParagraph && e is! StatefulElement;
    if (box != null && (tappable || text)) {
      final rect = MatrixUtils.transformRect(
        box.getTransformTo(null),
        Offset.zero & box.size,
      );
      if (!rect.isEmpty) {
        out.add((
          text ? '"${ro.text.toPlainText()}"' : '$w',
          rect,
        ));
      }
    }
    // Offstage routes and widgets are not seen.
    if (w is Offstage && w.offstage) return;
    if (w is Visibility && !w.visible) return;
    e.visitChildren(visit);
  }

  tester.binding.rootElement!.visitChildren(visit);
  return out;
}

void main() {
  late FakeClaxApi api;
  late String aid;
  late String tid;
  late Directory dir;

  setUp(() async {
    var t = 0;
    api = FakeClaxApi(
      clock: () => '2026-10-12T09:00:${(t++).toString().padLeft(2, '0')}Z',
    );
    aid = api.seedArtifact('Run log', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'install', pid: 1);
    tid = api.sendComment(aid, 'make the title blue').threadId;
    await api.replyAsAgent(aid, tid, 'Done: it is blue.');
    api.pushInbox('reply', artifactId: aid, threadId: tid, body: 'Added it');
    dir = Directory(
      '${scratchRoot().path}/mac-shots-${DateTime.now().microsecondsSinceEpoch}',
    )..createSync(recursive: true);
  });

  tearDown(() => dir.deleteSync(recursive: true));

  /// A test on the Mac.
  void mac(String description, WidgetTesterCallback body) =>
      testWidgets(description, body, variant: _onMac);

  /// [screen] pushed over a first route, so it has Back, in a 1200 x 800
  /// Mac window under its toolbar row, light or [dark].
  Future<void> pumpMac(
    WidgetTester tester,
    Widget screen, {
    bool dark = false,
  }) async {
    if (_shots) await tester.runAsync(loadFonts);
    tester.view.physicalSize = const Size(1200, 800) * 2;
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      Builder(
        builder: (context) {
          final data = MediaQuery.of(context);
          return WindowTitlebarScope(
            titlebar: _bar,
            child: MediaQuery(
              data: data.copyWith(
                padding: data.padding.copyWith(top: _bar.top),
                viewPadding: data.viewPadding.copyWith(top: _bar.top),
              ),
              child: Stack(
                key: const ValueKey('mac-window'),
                textDirection: TextDirection.ltr,
                children: [
                  MaterialApp(
                    debugShowCheckedModeBanner: false,
                    theme: claxTheme(
                      dark ? Brightness.dark : Brightness.light,
                      TargetPlatform.macOS,
                    ),
                    home: Builder(
                      builder: (context) => Center(
                        child: TextButton(
                          onPressed: () => Navigator.of(context).push(
                            MaterialPageRoute<void>(builder: (_) => screen),
                          ),
                          child: const Text('first'),
                        ),
                      ),
                    ),
                  ),
                  // The window buttons, in the shots only.
                  if (_shots)
                    const IgnorePointer(
                      child: CustomPaint(
                        size: Size.infinite,
                        painter: _WindowButtons(),
                      ),
                    ),
                ],
              ),
            ),
          );
        },
      ),
    );
    await tester.tap(find.text('first'));
    for (var i = 0; i < 20; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
  }

  Future<void> check(
    WidgetTester tester,
    String name, {
    bool dark = false,
  }) async {
    final corner = _bar.buttons;
    final inCorner = [
      for (final (what, rect) in _readOrClick(tester))
        if (rect.overlaps(corner)) '$what at $rect',
    ];
    expect(inCorner, isEmpty, reason: 'in the window buttons\' $corner');
    if (_shots) {
      final comparator = goldenFileComparator;
      final update = autoUpdateGoldenFiles;
      goldenFileComparator = LocalFileComparator(Uri.file('${_out!}/x.png'));
      autoUpdateGoldenFiles = true;
      try {
        await expectLater(
          find.byKey(const ValueKey('mac-window')),
          matchesGoldenFile('mac-$name-${dark ? 'dark' : 'light'}.png'),
        );
      } finally {
        goldenFileComparator = comparator;
        autoUpdateGoldenFiles = update;
      }
    }
  }

  mac('the inbox keeps out of the window buttons\' corner', (tester) async {
    await pumpMac(
      tester,
      InboxScreen(
        api: api,
        onOpen: (_) {},
        timer: (_, _) => _Timer(),
        now: () => DateTime.utc(2026, 10, 12, 9, 30),
        retryDelay: (_) async {},
        haikus: () async => const ['Old pond'],
      ),
    );
    expect(find.text('Inbox'), findsOneWidget);
    expect(find.byType(BackButton), findsOneWidget);
    // Back and the title are in the toolbar row, after the buttons.
    final back = tester.getRect(find.byType(BackButton));
    expect(back.left, greaterThanOrEqualTo(_bar.leading));
    expect(back.center.dy, closeTo(_bar.top / 2, 1));
    expect(
      tester.getRect(find.text('Inbox')).center.dy,
      closeTo(_bar.top / 2, 1),
    );
    await check(tester, 'inbox');
  });

  mac('settings keep out of the window buttons\' corner', (tester) async {
    await pumpMac(
      tester,
      SettingsScreen(
        store: SettingsStore(File('${dir.path}/settings.json')),
        secureKey: _Key(),
        cliSupported: false,
        timings: const <TimingRecord>[],
        haikus: () async => const ['Old pond'],
      ),
    );
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Settings'), findsWidgets);
    await check(tester, 'settings');
  });

  mac('a thread keeps out of the window buttons\' corner', (tester) async {
    await pumpMac(
      tester,
      ThreadDetail(
        api: api,
        aid: aid,
        tid: tid,
        voice: FakeVoice(),
        autoDictate: false,
        onPreview: (_) {},
        onRetry: (_) async => true,
        now: () => DateTime.utc(2026, 10, 12, 9, 20),
      ),
    );
    expect(find.text('make the title blue'), findsOneWidget);
    await check(tester, 'thread');
  });

  mac('the making screen keeps out of the window buttons\' corner', (
    tester,
  ) async {
    final plans = PlanStore();
    final link = BuildLink(plans: plans, control: FakeBuilds());
    late void Function(MakeUpdate) report;
    await pumpMac(
      tester,
      MakingScreen(
        request: 'a run tracker',
        make: (_, _, r) {
          report = r;
          return Completer<String>().future;
        },
        build: link,
        preview: _Preview.new,
        haikus: () async => const ['Old pond'],
      ),
    );
    report(const MakePlanKey('new:1'));
    await plans.set('new:1', const [
      PlanFeature(id: 'f1', title: 'File list', status: FeatureStatus.doing),
      PlanFeature(id: 'f2', title: 'Weekly chart'),
    ]);
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
    expect(find.byKey(const ValueKey('plan-sheet')), findsOneWidget);
    await check(tester, 'making');
  });

  for (final dark in [false, true]) {
    mac('an open app leaves the row to the window\'s toolbar, whose items '
        'it declares (${dark ? 'dark' : 'light'})', (tester) async {
      MacToolbar.reset();
      final declared = <Object?>[];
      final titles = <Object?>[];
      final m = tester.binding.defaultBinaryMessenger;
      m.setMockMethodCallHandler(MacToolbar.channel, (call) async {
        if (call.method == 'setItems') declared.add(call.arguments);
        return null;
      });
      m.setMockMethodCallHandler(MacWindow.channel, (call) async {
        if (call.method == 'setTitle') titles.add(call.arguments);
        return null;
      });
      addTearDown(() {
        m.setMockMethodCallHandler(MacToolbar.channel, null);
        m.setMockMethodCallHandler(MacWindow.channel, null);
      });
      final groceries = api.seedArtifact('Groceries', {
        'index.html': '<h1>Groceries</h1>',
      });
      final page = _Page();
      await pumpMac(
        tester,
        AppScreen(
          aid: groceries,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: page,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: const Stream.empty(),
          share: (_, _) async {},
        ),
        dark: dark,
      );
      page.push(
        Ready(artifact: groceries, version: 4, latest: 4, file: 'index.html'),
      );
      for (var i = 0; i < 10; i++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      final items = (declared.last! as Map)['items'] as List;
      expect([for (final i in items) (i as Map)['id']], [
        'back',
        'forward',
        'comment',
        'threads',
        'share',
      ]);
      expect((titles.last! as Map)['title'], 'Groceries');
      expect((titles.last! as Map)['subtitle'], 'Version 4');
      // Flutter draws nothing in the row: it is the toolbar's.
      final row = Offset.zero & Size(1200, _bar.top);
      expect([
        for (final (what, rect) in _readOrClick(tester))
          if (rect.overlaps(row)) '$what at $rect',
      ], isEmpty);
      expect(tester.getTopLeft(find.byKey(const ValueKey('page'))).dy, _bar.top);
      await check(tester, 'app', dark: dark);
      if (_shots) {
        File('${_out!}/mac-app-toolbar.json').writeAsStringSync(
          jsonEncode({'title': titles.last, 'items': items}),
        );
      }
    });
  }

  for (final dark in [false, true]) {
    mac('the launcher keeps out of the window buttons\' corner, its bar in '
        'the row (${dark ? 'dark' : 'light'})', (tester) async {
    await pumpMac(
      tester,
      Launcher(
        api: api,
        open: (_) {},
        make: (_, _, _) => Completer<String>().future,
        voice: FakeVoice.unsupported('No speech here'),
        openSettings: () {},
      ),
      dark: dark,
    );
    expect(find.text('Clax'), findsWidgets);
    // Its bar is the toolbar row: the title after the window buttons and
    // Inbox at the end, centred with them.
    final title = tester.getRect(find.text('Clax').first);
    expect(title.left, greaterThan(_bar.leading));
    expect(title.center.dy, closeTo(_bar.top / 2, 1));
    expect(
      tester.getCenter(find.byTooltip('Inbox')).dy,
      closeTo(_bar.top / 2, 1),
    );
    await check(tester, 'launcher', dark: dark);
  });
  }
}

/// Close, minimise and zoom as macOS 26 draws them in a 52-point toolbar
/// row: 14 points across, 23 points apart from 19 points in, centred in
/// the row.
class _WindowButtons extends CustomPainter {
  const _WindowButtons();

  @override
  void paint(Canvas canvas, Size size) {
    const colours = [Color(0xFFFF5F57), Color(0xFFFEBC2E), Color(0xFF28C840)];
    for (var i = 0; i < 3; i++) {
      canvas.drawCircle(
        Offset(_bar.leading - 7 - 23.0 * (2 - i), _bar.top / 2),
        7,
        Paint()..color = colours[i],
      );
    }
  }

  @override
  bool shouldRepaint(_WindowButtons old) => false;
}
