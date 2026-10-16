import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

const _page = {'index.html': '<h1>x</h1>'};

class _Actions implements HomeActions {
  final opened = <String>[];
  @override
  void open(String id) => opened.add(id);
  @override
  void openMake(String id) {}
  @override
  void newApp() {}
  @override
  void menu(String id, Rect anchor) {}
  @override
  void openInbox() {}
  @override
  void openSettings() {}
  @override
  void importApp() {}
}

void main() {
  late FakeClaxApi api;
  late String home;
  late String other;
  late HomeHost host;
  late _Actions actions;

  setUp(() {
    var t = 0;
    api = FakeClaxApi(
      clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
    );
    home = api.seedArtifact('Home', _page);
    other = api.seedArtifact('Mimic', _page, capabilities: {'home': {}});
    actions = _Actions();
    host = HomeHost(
      api: api,
      stream: api.stream(),
      closeStream: true,
      makes: MakeService((r, c, s) => Completer<String>().future),
      builds: null,
      settings: HomeSettings(homeAppId: home),
      actions: actions,
    );
  });

  tearDown(() => host.dispose());

  Future<FakeAppSurface> pump(WidgetTester tester, String aid) async {
    final surface = FakeAppSurface();
    await tester.pumpWidget(
      MaterialApp(
        home: AppScreen(
          key: ValueKey(aid),
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: const Stream.empty(),
          home: host,
        ),
      ),
    );
    await tester.pump();
    return surface;
  }

  DeviceRequest req(
    String id,
    String method, [
    List<Object?> args = const [],
  ]) => DeviceRequest(id: id, cap: 'home', method: method, args: args);

  Future<List<DeviceResult>> ask(
    WidgetTester tester,
    FakeAppSurface surface,
    List<DeviceRequest> rs,
  ) async {
    rs.forEach(surface.push);
    await tester.pump();
    await tester.pump();
    return surface.hostCalls.whereType<DeviceResult>().toList();
  }

  testWidgets('the home app is served home: its calls and its events', (
    tester,
  ) async {
    final surface = await pump(tester, home);
    final results = await ask(tester, surface, [
      req('a', 'available'),
      req('b', 'apps'),
      req('c', 'open', [
        {'id': other},
      ]),
    ]);
    expect(results.map((r) => (r.id, r.code)), [
      ('a', null),
      ('b', null),
      ('c', null),
    ]);
    expect(results[0].value, isTrue);
    expect(
      [for (final a in results[1].value as List) (a as Map)['id']],
      [other],
    );
    expect(actions.opened, [other]);
    api.pushInbox('reply', artifactId: other, body: 'Done');
    await tester.pump();
    await tester.pump();
    final told = surface.hostCalls.whereType<HomeDeviceEvent>();
    expect(told.map((e) => (e.name, e.data)), contains(('unread', 1)));
  });

  testWidgets('another app declaring home sees it unavailable and is refused, '
      'after the home app was open too', (tester) async {
    final first = await pump(tester, home);
    await ask(tester, first, [req('a', 'apps')]);
    final surface = await pump(tester, other);
    final results = await ask(tester, surface, [
      req('a', 'available'),
      req('b', 'apps'),
      req('c', 'open', [
        {'id': home},
      ]),
    ]);
    expect(results.map((r) => (r.id, r.code)), [
      ('a', null),
      ('b', 'refused'),
      ('c', 'refused'),
    ]);
    expect(results[0].value, isFalse);
    expect(actions.opened, isEmpty);
    api.pushInbox('reply', artifactId: other, body: 'Done');
    await tester.pump();
    expect(surface.hostCalls.whereType<HomeDeviceEvent>(), isEmpty);
  });

  testWidgets('a malformed call is a bad request', (tester) async {
    final surface = await pump(tester, home);
    final results = await ask(tester, surface, [
      req('a', 'open', ['not an object']),
      req('b', 'open', [<String, Object?>{}]),
    ]);
    expect(results.map((r) => r.code), ['bad_request', 'bad_request']);
  });
}
