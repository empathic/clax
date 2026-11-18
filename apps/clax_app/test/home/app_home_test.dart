import 'dart:io';

import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_cover.dart';
import 'package:clax_app/ui/comment_bar.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/loop_support.dart';
import '../support/fake_core.dart';
import '../support/fake_surface.dart';
import '../support/fake_voice.dart';
import '../support/settle.dart';

const _page = {'index.html': '<h1>x</h1>'};

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';

  @override
  Future<void> write(String? key) async {}
}

/// Whether [page] has been answered its device request [id].
bool _answered(FakeAppSurface page, String id) =>
    page.hostCalls.whereType<DeviceResult>().any((r) => r.id == id);

/// Ends the app, and lets the home route's 10 s wait for `shown` run out.
Future<void> _finish(WidgetTester tester) async {
  await tester.pumpWidget(const SizedBox());
  await tester.pump(const Duration(seconds: 11));
}

void main() {
  late Directory dir;
  late FakeClaxApi api;
  late FakeCoreHost coreHost;
  late List<FakeAppSurface> surfaces;
  late RecordingTimings timings;
  late String home;
  late String notes;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('app-home-');
    var t = 0;
    api = FakeClaxApi(
      clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
    );
    coreHost = FakeCoreHost();
    surfaces = [];
    timings = RecordingTimings();
    home = api.seedArtifact('Home', _page, capabilities: {'home': {}});
    notes = api.seedArtifact('Notes', _page);
  });
  tearDown(() => dir.deleteSync(recursive: true));

  AppServices services({HomeSettings? homeSettings}) => AppServices(
    homeSettings: homeSettings,
    core: coreHost,
    api: (_) => api,
    backend: (s, k) => HeldBackend([], name: backendMessagesApi),
    voice: FakeVoice.unsupported('No speech here'),
    settings: SettingsStore(File('${dir.path}/settings.json')),
    key: _Key(),
    timings: timings,
    agentTimings: RecordingTimings(),
    surface: () {
      final s = FakeAppSurface();
      surfaces.add(s);
      return s;
    },
    installId: 'install',
    supportDir: dir.path,
    prompts: PromptBuilder(rootBundle),
    pid: 42,
  );

  Future<void> recordHome(WidgetTester tester) => tester.runAsync(
    () => (HomeSettings.loadSync(Directory(dir.path))..homeAppId = home).save(),
  );

  /// Starts the app with [home] as its home app, up and ready.
  Future<FakeAppSurface> start(WidgetTester tester) async {
    await recordHome(tester);
    await tester.pumpWidget(
      AppRoot(
        services: services(),
      ),
    );
    coreHost.set(const CoreReady());
    await settleUntil(tester, () => surfaces.isNotEmpty, what: 'the home page');
    final page = surfaces.single;
    page.push(Ready(artifact: home, version: 1, latest: 1, file: 'index.html'));
    page.push(
      const DeviceRequest(id: 'r1', cap: 'home', method: 'apps', args: []),
    );
    await settleUntil(
      tester,
      () => _answered(page, 'r1'),
      what: 'the home page’s apps answered',
    );
    return page;
  }

  testWidgets('another app taking its top level to the home app goes back '
      'to the home screen instead of opening it again', (tester) async {
    final page = await start(tester);
    page.push(
      DeviceRequest(
        id: 'r2',
        cap: 'home',
        method: 'open',
        args: [
          {'id': notes},
        ],
      ),
    );
    await settleUntil(tester, () => surfaces.length == 2, what: 'Notes opened');
    final notesPage = surfaces[1];
    notesPage.push(
      Ready(artifact: notes, version: 1, latest: 1, file: 'index.html'),
    );

    notesPage.navigateElsewhere(home);
    await settleUntil(tester, () => notesPage.disposed, what: 'Notes closed');
    expect(surfaces, hasLength(2), reason: 'no second home page');
    expect(notesPage.disposed, isTrue, reason: 'Notes closed');
    expect(page.disposed, isFalse);
    await _finish(tester);
  });

  testWidgets("Settings › Open in comment mode takes the home screen's own "
      'page into comment mode', (tester) async {
    final page = await start(tester);
    page.push(
      const DeviceRequest(
        id: 'r2',
        cap: 'home',
        method: 'openSettings',
        args: [],
      ),
    );
    await settle(tester, find.byType(SettingsScreen));
    expect(find.byType(CommentBar), findsNothing);
    await tester.scrollUntilVisible(
      find.text('Reset to Clax’s home screen'),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    expect(find.text('Home screen'), findsOneWidget);
    await tester.tap(find.text('Open in comment mode'));
    await settle(tester, find.byType(CommentBar));
    expect(find.text('Home screen'), findsNothing, reason: 'Settings closed');
    expect(surfaces, hasLength(1), reason: 'no second home page');
    expect(find.byType(CommentBar), findsOneWidget);
    await _finish(tester);
  });

  testWidgets('home settings read before the start are the ones the home '
      'screen uses: no file is read for them', (tester) async {
    await recordHome(tester);
    final early = await tester.runAsync(
      () => HomeSettings.load(Directory(dir.path)),
    );
    // The file says there is no home app by the time the app starts.
    File('${dir.path}/${HomeSettings.fileName}').writeAsStringSync('{}');
    await tester.pumpWidget(AppRoot(services: services(homeSettings: early)));
    coreHost.set(const CoreReady());
    await settleUntil(tester, () => surfaces.isNotEmpty, what: 'the home page');
    expect(surfaces, isNotEmpty, reason: 'the home app''s page');
    expect(surfaces.first.loads.single.path, contains(home));
    await _finish(tester);
  });

  testWidgets("a launch shows no image of the home screen: the home app's "
      'page comes under its loading cover, and home-usable is timed at its '
      'first shown', (tester) async {
    await recordHome(tester);
    timings.start(homeUsable);
    await tester.pumpWidget(AppRoot(services: services()));
    expect(find.byType(Image), findsNothing);
    coreHost.set(const CoreReady());
    await settleUntil(tester, () => surfaces.isNotEmpty, what: 'the home page');
    expect(find.byKey(const ValueKey('app-cover')), findsOneWidget);
    final page = surfaces.single;
    page.push(Ready(artifact: home, version: 1, latest: 1, file: 'index.html'));
    page.push(
      const DeviceRequest(id: 'r1', cap: 'home', method: 'shown', args: []),
    );
    await settleUntil(
      tester,
      () => timings.stopped.isNotEmpty,
      what: 'home-usable timed',
    );
    await tester.pump(AppCover.markHold);
    expect(find.byKey(const ValueKey('app-cover')), findsNothing);
    expect(timings.stopped.single.name, homeUsable);
    expect(timings.stopped.single.extra, {'artifact': home});
    await _finish(tester);
  });

  testWidgets('a launch that shows the built-in home screen records no '
      'home-usable, not even at a later shown', (tester) async {
    await recordHome(tester);
    timings.start(homeUsable);
    await tester.pumpWidget(AppRoot(services: services()));
    coreHost.set(const CoreReady());
    await settleUntil(tester, () => surfaces.isNotEmpty, what: 'the home page');
    final page = surfaces.single;
    page.push(Ready(artifact: home, version: 1, latest: 1, file: 'index.html'));
    // No shown within 10 s: the built-in home screen.
    await tester.pump(const Duration(seconds: 11));
    page.push(
      const DeviceRequest(id: 'r1', cap: 'home', method: 'shown', args: []),
    );
    await settleUntil(
      tester,
      () => _answered(page, 'r1'),
      what: 'the shown request answered',
    );
    expect(timings.stopped.where((r) => r.name == homeUsable), isEmpty);
    await _finish(tester);
  });
}
