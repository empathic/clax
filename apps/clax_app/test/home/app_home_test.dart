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
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/loop_support.dart';
import '../support/fake_core.dart';
import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

const _page = {'index.html': '<h1>x</h1>'};

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';

  @override
  Future<void> write(String? key) async {}
}

/// Pumps until the app's async work settles; app pages are given their
/// `ready` so no progress indicator keeps [WidgetTester.pumpAndSettle]
/// from ending.
Future<void> _settle(WidgetTester tester) async {
  for (var i = 0; i < 20; i++) {
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pump(const Duration(milliseconds: 50));
  }
}

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

  AppServices services() => AppServices(
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
    await _settle(tester);
    final page = surfaces.single;
    page.push(Ready(artifact: home, version: 1, latest: 1, file: 'index.html'));
    page.push(
      const DeviceRequest(id: 'r1', cap: 'home', method: 'apps', args: []),
    );
    await _settle(tester);
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
    await _settle(tester);
    expect(surfaces, hasLength(2), reason: 'Notes opened over home');
    final notesPage = surfaces[1];
    notesPage.push(
      Ready(artifact: notes, version: 1, latest: 1, file: 'index.html'),
    );
    await _settle(tester);

    notesPage.navigateElsewhere(home);
    await _settle(tester);
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
    await _settle(tester);
    expect(find.byType(CommentBar), findsNothing);
    await tester.scrollUntilVisible(
      find.text('Reset to Clax’s home screen'),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    expect(find.text('Home screen'), findsOneWidget);
    await tester.tap(find.text('Open in comment mode'));
    await _settle(tester);
    expect(find.text('Home screen'), findsNothing, reason: 'Settings closed');
    expect(surfaces, hasLength(1), reason: 'no second home page');
    expect(find.byType(CommentBar), findsOneWidget);
    await _finish(tester);
  });

  testWidgets('a launch shows the splash while the core opens, then the '
      "home app's page under its loading cover, and times home-usable at its "
      'first shown', (tester) async {
    await recordHome(tester);
    timings.start(homeUsable);
    await tester.pumpWidget(AppRoot(services: services()));
    expect(find.byKey(const ValueKey('splash')), findsOneWidget);
    expect(find.byType(Image), findsNothing);
    coreHost.set(const CoreReady());
    await _settle(tester);
    expect(find.byKey(const ValueKey('app-cover')), findsOneWidget);
    final page = surfaces.single;
    page.push(Ready(artifact: home, version: 1, latest: 1, file: 'index.html'));
    page.push(
      const DeviceRequest(id: 'r1', cap: 'home', method: 'shown', args: []),
    );
    await _settle(tester);
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
    await _settle(tester);
    final page = surfaces.single;
    page.push(Ready(artifact: home, version: 1, latest: 1, file: 'index.html'));
    // No shown within 10 s: the built-in home screen.
    await tester.pump(const Duration(seconds: 11));
    await _settle(tester);
    page.push(
      const DeviceRequest(id: 'r1', cap: 'home', method: 'shown', args: []),
    );
    await _settle(tester);
    expect(timings.stopped.where((r) => r.name == homeUsable), isEmpty);
    await _finish(tester);
  });
}
