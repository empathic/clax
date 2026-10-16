import 'dart:io';

import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/home/home_snapshot.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/comment_bar.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/loop_support.dart';
import '../support/fake_supervisor.dart';
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
  late FakeSupervisor supervisor;
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
    supervisor = FakeSupervisor();
    surfaces = [];
    timings = RecordingTimings();
    home = api.seedArtifact('Home', _page, capabilities: {'home': {}});
    notes = api.seedArtifact('Notes', _page);
  });
  tearDown(() => dir.deleteSync(recursive: true));

  AppServices services({HomeSnapshot? snapshot, HomeSnapshotStore? store}) =>
      AppServices(
        supervisor: supervisor,
        api: (_) => api,
        backend: (s, k) => HeldBackend([], name: backendMessagesApi),
        voice: FakeVoice.unsupported('No speech here'),
        settings: SettingsStore(File('${dir.path}/settings.json')),
        key: _Key(),
        timings: timings,
        agentTimings: RecordingTimings(),
        surface: (_) {
          final s = FakeAppSurface();
          surfaces.add(s);
          return s;
        },
        installId: 'install',
        supportDir: dir.path,
        prompts: PromptBuilder(rootBundle),
        homeSnapshot: snapshot,
        homeSnapshots: store,
        pid: 42,
      );

  Future<void> recordHome(WidgetTester tester) => tester.runAsync(
    () => (HomeSettings.loadSync(Directory(dir.path))..homeAppId = home).save(),
  );

  /// Starts the app with [home] as its home app, up and ready.
  Future<FakeAppSurface> start(
    WidgetTester tester, {
    HomeSnapshotStore? store,
  }) async {
    await recordHome(tester);
    await tester.pumpWidget(AppRoot(services: services(store: store)));
    supervisor.set(Up(endpoint()));
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

  testWidgets("Settings › Open takes the home screen's own page into "
      'comment mode', (tester) async {
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
      find.text('Reset to Clax\'s home screen'),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    expect(find.text('Home screen'), findsOneWidget);
    await tester.tap(find.text('Open'));
    await _settle(tester);
    expect(find.text('Home screen'), findsNothing, reason: 'Settings closed');
    expect(surfaces, hasLength(1), reason: 'no second home page');
    expect(find.byType(CommentBar), findsOneWidget);
    await _finish(tester);
  });

  testWidgets('a launch shows the kept snapshot before the daemon is up, and '
      "times home-cover and home-usable", (tester) async {
    await recordHome(tester);
    timings
      ..start(homeCover)
      ..start(homeUsable);
    final snapshot = HomeSnapshot(
      aid: home,
      version: 1,
      png: Uint8List(0),
      width: 800,
      height: 600,
      tiles: const [],
      at: DateTime.utc(2026, 10, 16),
    );
    await tester.pumpWidget(AppRoot(services: services(snapshot: snapshot)));
    expect(find.byKey(const ValueKey('launch-snapshot')), findsOneWidget);
    await tester.pump();
    expect(timings.stopped.map((r) => r.name).toSet(), {homeCover});

    supervisor.set(Up(endpoint()));
    await _settle(tester);
    expect(find.byKey(const ValueKey('home-snapshot-cover')), findsOneWidget);
    final page = surfaces.single;
    page.push(
      const DeviceRequest(id: 'r1', cap: 'home', method: 'shown', args: []),
    );
    await _settle(tester);
    expect(find.byKey(const ValueKey('home-snapshot-cover')), findsNothing);
    // (These test timings keep a name started once it stops.)
    expect(timings.stopped.map((r) => r.name).toSet(), {homeCover, homeUsable});
    expect(timings.stopped.last.extra, {'artifact': home, 'snapshot': true});
    await _finish(tester);
  });

  testWidgets('a launch that shows the built-in home screen records no '
      'home-usable, not even at a later shown', (tester) async {
    await recordHome(tester);
    timings.start(homeUsable);
    await tester.pumpWidget(AppRoot(services: services()));
    supervisor.set(Up(endpoint()));
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

  HomeSnapshot kept({int version = 1, DateTime? at}) => HomeSnapshot(
    aid: home,
    version: version,
    png: Uint8List(0),
    width: 800,
    height: 600,
    tiles: const [],
    at: at ?? DateTime.now(),
  );

  testWidgets('a snapshot older than a week is not shown, and is cleared', (
    tester,
  ) async {
    await recordHome(tester);
    final store = HomeSnapshotStore(Directory('${dir.path}/cache'));
    final old = kept(at: DateTime.now().subtract(const Duration(days: 8)));
    await tester.runAsync(() => store.save(old));
    await tester.pumpWidget(
      AppRoot(
        services: services(snapshot: old, store: store),
      ),
    );
    expect(find.byKey(const ValueKey('launch-snapshot')), findsNothing);
    await tester.runAsync(() async {
      expect(await store.load(decode: false), isNull);
    });
    await _finish(tester);
  });

  testWidgets('a page of another version than the snapshot takes it away '
      'and clears it', (tester) async {
    await recordHome(tester);
    final store = HomeSnapshotStore(Directory('${dir.path}/cache'));
    final snapshot = kept(version: 1);
    await tester.runAsync(() => store.save(snapshot));
    await tester.pumpWidget(
      AppRoot(
        services: services(snapshot: snapshot, store: store),
      ),
    );
    supervisor.set(Up(endpoint()));
    await _settle(tester);
    expect(find.byKey(const ValueKey('home-snapshot-cover')), findsOneWidget);
    surfaces.single.push(
      Ready(artifact: home, version: 2, latest: 2, file: 'index.html'),
    );
    await _settle(tester);
    expect(find.byKey(const ValueKey('home-snapshot-cover')), findsNothing);
    await tester.runAsync(() async {
      await Future<void>.delayed(Duration.zero);
      expect(await store.load(decode: false), isNull);
    });
    await _finish(tester);
  });

  testWidgets('choosing the built-in home screen clears the snapshot', (
    tester,
  ) async {
    final store = HomeSnapshotStore(Directory('${dir.path}/cache'));
    await tester.runAsync(() => store.save(kept()));
    final page = await start(tester, store: store);
    page.push(
      const DeviceRequest(
        id: 'r2',
        cap: 'home',
        method: 'openSettings',
        args: [],
      ),
    );
    await _settle(tester);
    await tester.scrollUntilVisible(
      find.text('Use the built-in home screen'),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    final toggle = find.widgetWithText(
      SwitchListTile,
      'Use the built-in home screen',
    );
    await tester.ensureVisible(toggle);
    await tester.pumpAndSettle();
    await tester.tap(toggle);
    await _settle(tester);
    expect(
      tester
          .widget<SwitchListTile>(
            find.widgetWithText(SwitchListTile, 'Use the built-in home screen'),
          )
          .value,
      isTrue,
    );
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(await store.load(decode: false), isNull);
    });
    await _finish(tester);
  });
}
