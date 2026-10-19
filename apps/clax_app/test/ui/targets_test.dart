// Every control on the phone's main screens is at least 48×48 dp to touch
// (Android's tap-target guideline): the launcher with its banners and a
// tile's menu, an open app with and without a change's chip, comment mode
// with its bar and its composer, the inbox, a thread and Settings, each in
// the app's theme on an Android phone.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_route.dart' show HomeBanner;
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/platform/app_notice.dart' show AppAction;
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:clax_app/ui/tile_label.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

const _page = {'index.html': '<h1>Runs</h1>'};

/// Android's 48×48 dp tap-target guideline, measuring a text field by the
/// tappable surface around it where there is one: a search bar's text is
/// 24 dp high, and a tap anywhere on the bar's 56 dp focuses it.
class _Targets extends MinimumTapTargetGuideline {
  const _Targets()
    : super(
        size: const Size(48, 48),
        link: 'https://support.google.com/accessibility/android/answer/7101858',
      );

  @override
  bool shouldSkipNode(SemanticsNode node) {
    if (super.shouldSkipNode(node)) return true;
    if (!node.getSemanticsData().flagsCollection.isTextField) return false;
    for (var p = node.parent; p != null; p = p.parent) {
      if (p.getSemanticsData().hasAction(SemanticsAction.tap)) {
        return p.rect.width >= size.width && p.rect.height >= size.height;
      }
    }
    return false;
  }
}

const _guideline = _Targets();

class _Key implements SecureKey {
  _Key(this.value);
  String? value;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

/// [home] in the app's theme on an Android phone, light or [dark].
Future<void> _pump(WidgetTester tester, Widget home, {bool dark = false}) async {
  tester.view.physicalSize = const Size(1080, 2400);
  tester.view.devicePixelRatio = 2.625;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MaterialApp(
      theme: claxTheme(Brightness.light, TargetPlatform.android),
      darkTheme: claxTheme(Brightness.dark, TargetPlatform.android),
      themeMode: dark ? ThemeMode.dark : ThemeMode.light,
      home: home,
    ),
  );
}

Future<void> _settle(WidgetTester tester) async {
  await tester.runAsync(() => Future<void>.delayed(Duration.zero));
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 100));
  }
}

void main() {
  testWidgets('the launcher, with its banners and a tile’s menu', (
    tester,
  ) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T00:00:00Z');
    api.seedArtifact('Runs', _page);
    api.seedArtifact('Groceries', _page);
    await _pump(
      tester,
      Launcher(
        api: api,
        open: (_) {},
        make: (r, c, s) => throw StateError('unused'),
        voice: FakeVoice.unsupported('No speech here'),
        openSettings: () {},
        needsKey: true,
        banner: HomeBanner(undo: () async => null, reset: () async => null),
      ),
    );
    await _settle(tester);
    await expectLater(tester, meetsGuideline(_guideline));
    await tester.longPress(
      find.byWidgetPredicate((w) => w is TileLabel && w.text == 'Runs'),
    );
    await _settle(tester);
    expect(find.text('Inspect'), findsOneWidget);
    await expectLater(tester, meetsGuideline(_guideline));
  });

  for (final dark in [false, true]) {
    testWidgets('an open app (${dark ? 'dark' : 'light'})', (tester) async {
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Runs', _page);
      final surface = FakeAppSurface();
      final worker = StreamController<WorkerEvent>.broadcast(sync: true);
      addTearDown(worker.close);
      await _pump(
        tester,
        AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: worker.stream,
        ),
        dark: dark,
      );
      surface.push(
        const Ready(artifact: 'x', version: 1, latest: 1, file: 'index.html'),
      );
      await _settle(tester);
      await expectLater(tester, meetsGuideline(_guideline));
      // A change under way, with the Clax pill.
      surface.push(const Sent('thread-1'));
      await _settle(tester);
      expect(find.byKey(const ValueKey('clax-pill')), findsOneWidget);
      await expectLater(tester, meetsGuideline(_guideline));
      // How it ended, with its chip.
      worker.add(WorkerFailed(aid, 'it broke'));
      await _settle(tester);
      expect(find.byKey(const ValueKey('chip-dismiss')), findsOneWidget);
      await expectLater(tester, meetsGuideline(_guideline));
      // The chip's delayed open runs out after the screen goes.
      await tester.pumpWidget(const SizedBox());
      await tester.pump(const Duration(seconds: 5));
    });
  }

  for (final (width, dark) in [(412.0, false), (412.0, true), (340.0, false)]) {
    testWidgets('comment mode: the bar, then the composer '
        '(${width.toInt()} dp, ${dark ? 'dark' : 'light'})', (tester) async {
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Runs', _page);
      final surface = FakeAppSurface();
      final actions = StreamController<AppAction>.broadcast(sync: true);
      addTearDown(actions.close);
      final worker = StreamController<WorkerEvent>.broadcast(sync: true);
      addTearDown(worker.close);
      await _pump(
        tester,
        AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: worker.stream,
          actions: actions.stream,
        ),
        dark: dark,
      );
      tester.view.physicalSize = Size(width * 2.625, 2400);
      surface.push(
        const Ready(artifact: 'x', version: 1, latest: 1, file: 'index.html'),
      );
      await _settle(tester);
      actions.add(AppAction.comment);
      await _settle(tester);
      expect(find.byKey(const ValueKey('comment-bar')), findsOneWidget);
      await expectLater(tester, meetsGuideline(_guideline));
      // A pick opens the composer.
      surface.push(
        const StateMsg(
          commenting: true,
          draft: Draft(
            key: 'p1',
            label: '“Runs”',
            quote: 'Runs',
            text: '',
            clip: DraftClip.none,
          ),
          busy: false,
        ),
      );
      await _settle(tester);
      expect(find.byKey(const ValueKey('composer')), findsOneWidget);
      await expectLater(tester, meetsGuideline(_guideline));
    });
  }

  testWidgets('the inbox', (tester) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T08:00:00Z');
    final runs = api.seedArtifact('Runs', _page);
    final budget = api.seedArtifact('Household budget', _page);
    api.pushInbox('version', artifactId: budget, versionN: 4, note: 'Totals');
    api.pushInbox(
      'reply',
      artifactId: runs,
      threadId: 't1',
      body: 'Done. Distances now read in km.',
    );
    await _pump(
      tester,
      InboxScreen(
        api: api,
        onOpen: (_) {},
        now: () => DateTime.utc(2026, 10, 12, 9, 30),
        haikus: () async => const [],
      ),
    );
    await _settle(tester);
    await expectLater(tester, meetsGuideline(_guideline));
  });

  testWidgets('a thread', (tester) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
    final aid = api.seedArtifact('Runs', _page);
    await api.registerAppSession(installId: 'install', pid: 1);
    final item = api.sendComment(
      aid,
      'make the title blue',
      anchor: const {'kind': 'text', 'file': 'index.html', 'quote': 'Runs'},
    );
    await api.replyAsAgent(aid, item.threadId, 'Done: it is blue.');
    await _pump(
      tester,
      ThreadDetail(
        api: api,
        aid: aid,
        tid: item.threadId,
        voice: FakeVoice(),
        autoDictate: false,
        onPreview: (_) {},
        onRetry: (_) async => true,
        now: () => DateTime.utc(2026, 10, 12, 9, 20),
      ),
    );
    await _settle(tester);
    await expectLater(tester, meetsGuideline(_guideline));
  });

  testWidgets('Settings', (tester) async {
    final dir = Directory.systemTemp.createTempSync('clax-targets-');
    addTearDown(() => dir.deleteSync(recursive: true));
    final store = SettingsStore(File('${dir.path}/settings.json'));
    await tester.runAsync(store.load);
    await _pump(
      tester,
      SettingsScreen(
        store: store,
        secureKey: _Key('sk-ant-api03-targets'),
        cliSupported: false,
        timings: const [],
      ),
    );
    await _settle(tester);
    await expectLater(tester, meetsGuideline(_guideline));
    // The rest of the page, scrolled into view.
    await tester.drag(find.byType(Scrollable).first, const Offset(0, -3000));
    await _settle(tester);
    await expectLater(tester, meetsGuideline(_guideline));
  });
}
