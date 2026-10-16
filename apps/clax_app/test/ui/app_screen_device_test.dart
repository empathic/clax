import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../platform/device/device_host_test.dart'
    show
        FakeCamera,
        FakeConsent,
        FakeFiles,
        FakeLocation,
        FakeNotifier,
        FakeShare;
import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

void main() {
  Future<FakeAppSurface> pump(WidgetTester tester, DeviceHost? device) async {
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Plants', {'index.html': '<h1>Plants</h1>'});
    final surface = FakeAppSurface();
    await tester.pumpWidget(
      MaterialApp(
        home: AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: const Stream.empty(),
          device: device,
        ),
      ),
    );
    return surface;
  }

  testWidgets(
    'a device call is made by the device host and answered in the page',
    (tester) async {
      final consent = FakeConsent();
      final device = DeviceHost(
        grants: MemoryDeviceGrants(),
        consent: consent,
        reminders: Reminders(FakeNotifier()),
        share: FakeShare(),
        files: FakeFiles(),
        camera: FakeCamera(),
        location: FakeLocation(),
      );
      final surface = await pump(tester, device);
      surface.push(
        const DeviceRequest(
          id: 'd7',
          cap: 'location',
          method: 'current',
          args: [],
        ),
      );
      await tester.pump();
      await tester.pump();
      final result = surface.hostCalls.whereType<DeviceResult>().single;
      expect((result.id, result.code), ('d7', null));
      expect(result.value, {
        'lat': 52.5,
        'lon': 13.4,
        'accuracy': 12.0,
        'precise': true,
      });
      expect(consent.asked.single.$2, 'location');
    },
  );

  DeviceHost host(FakeLocation location) => DeviceHost(
    grants: MemoryDeviceGrants(),
    consent: FakeConsent(),
    reminders: Reminders(FakeNotifier()),
    share: FakeShare(),
    files: FakeFiles(),
    camera: FakeCamera(),
    location: location,
  );

  DeviceRequest watch(String id) => DeviceRequest(
    id: 'req-$id',
    cap: 'location',
    method: 'watch',
    args: [
      {'watch': id},
    ],
  );

  testWidgets(
    'a watch shows the location pill, sends updates, and stops for good '
    'when the app goes to the background',
    (tester) async {
      final location = FakeLocation();
      final surface = await pump(tester, host(location));
      surface.push(watch('w'));
      await tester.pump();
      await tester.pump();
      await tester.pump();
      expect(surface.hostCalls.whereType<DeviceResult>().single.value, {
        'watching': true,
      });
      expect(find.byKey(const ValueKey('location-pill')), findsOneWidget);
      location.move(52.5, 13.4);
      await tester.pump();
      expect(surface.hostCalls.whereType<DeviceEvent>().single.value, {
        'lat': 52.5,
        'lon': 13.4,
        'accuracy': 8.0,
        'precise': true,
      });

      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pump();
      final end = surface.hostCalls.whereType<DeviceEvent>().last;
      expect((end.watch, end.code), ('w', 'stopped'));
      expect(location.stream, isNull);
      expect(find.byKey(const ValueKey('location-pill')), findsNothing);

      // Back in front: no watch starts again by itself.
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      expect(location.stream, isNull);
      expect(location.asked, hasLength(1));
      expect(find.byKey(const ValueKey('location-pill')), findsNothing);
    },
  );

  // The pill's screenshot, off by default; run with
  //   CLAX_L1_SHOTS=1 flutter test --update-goldens test/ui/app_screen_device_test.dart
  // and look at target/flutter-app/l1/location-pill-*.png.
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    testWidgets(
      'location pill, phone, $scheme',
      skip: Platform.environment['CLAX_L1_SHOTS'] != '1',
      (tester) async {
        await tester.runAsync(() async {
          final dir =
              '${Platform.environment['FLUTTER_ROOT']}/bin/cache/artifacts/material_fonts';
          Future<ByteData> f(String n) async =>
              ByteData.sublistView(await File('$dir/$n').readAsBytes());
          await (FontLoader(
            'MaterialIcons',
          )..addFont(f('MaterialIcons-Regular.otf'))).load();
        });
        tester.view.physicalSize = const Size(400, 300);
        tester.view.devicePixelRatio = 1;
        tester.view.padding = const FakeViewPadding(top: 32);
        addTearDown(tester.view.reset);
        final api = FakeClaxApi();
        final aid = api.seedArtifact('Plants', {'index.html': '<h1>P</h1>'});
        final surface = FakeAppSurface();
        await tester.pumpWidget(
          MaterialApp(
            debugShowCheckedModeBanner: false,
            theme: claxTheme(dark ? Brightness.dark : Brightness.light),
            home: AppScreen(
              aid: aid,
              base: Uri.parse('http://127.0.0.1:7999/'),
              api: api,
              surface: surface,
              voice: FakeVoice(),
              settings: AppSettings(),
              timings: const NoTimings(),
              worker: const Stream.empty(),
              device: host(FakeLocation()),
            ),
          ),
        );
        surface.push(watch('w'));
        for (var i = 0; i < 4; i++) {
          await tester.pump();
        }
        await expectLater(
          find.byType(MaterialApp),
          matchesGoldenFile(
            '../../../../target/flutter-app/l1/location-pill-$scheme.png',
          ),
        );
      },
    );
  }

  for (final (name, pad) in const [
    ('a tall status bar', EdgeInsets.only(top: 48)),
    ('a cutout', EdgeInsets.only(top: 56)),
    ('a landscape cutout', EdgeInsets.only(top: 24, left: 44)),
  ]) {
    testWidgets('the location pill sits under the top inset and inside the '
        'side insets: $name', (tester) async {
      tester.view.physicalSize = const Size(780, 780);
      tester.view.devicePixelRatio = 1;
      tester.view.padding = FakeViewPadding(top: pad.top, left: pad.left);
      addTearDown(tester.view.reset);
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Plants', {'index.html': '<h1>P</h1>'});
      final surface = FakeAppSurface();
      await tester.pumpWidget(
        MaterialApp(
          home: AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999/'),
            api: api,
            surface: surface,
            voice: FakeVoice(),
            settings: AppSettings(),
            timings: const NoTimings(),
            worker: const Stream.empty(),
            device: host(FakeLocation()),
          ),
        ),
      );
      surface.push(watch('w'));
      for (var i = 0; i < 4; i++) {
        await tester.pump();
      }
      final pill = tester.getRect(find.byKey(const ValueKey('location-pill')));
      expect(pill.top, greaterThanOrEqualTo(pad.top));
      expect(pill.left, greaterThanOrEqualTo(pad.left));
    });
  }

  testWidgets('a tap on the location pill stops location', (tester) async {
    final location = FakeLocation();
    final surface = await pump(tester, host(location));
    surface.push(watch('w'));
    await tester.pump();
    await tester.pump();
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('location-pill')));
    await tester.pump();
    expect(surface.hostCalls.whereType<DeviceEvent>().last.code, 'stopped');
    expect(location.stream, isNull);
    expect(find.byKey(const ValueKey('location-pill')), findsNothing);
  });

  testWidgets(
    'location that Android will not ask for again offers Open settings',
    (tester) async {
      final location = FakeLocation()
        ..error = const DeviceError('system_denied', 'never again');
      final surface = await pump(tester, host(location));
      surface.push(
        const DeviceRequest(
          id: 'd1',
          cap: 'location',
          method: 'current',
          args: [],
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(
        surface.hostCalls.whereType<DeviceResult>().single.code,
        'system_denied',
      );
      await tester.pump(const Duration(milliseconds: 500));
      expect(
        find.text('Location is off for Clax in Android settings'),
        findsOneWidget,
      );
      await tester.tap(find.text('Open settings'));
      await tester.pump();
      expect(location.opened, [LocationSettingsPage.app]);
    },
  );

  testWidgets('without a device host every device call is unavailable', (
    tester,
  ) async {
    final surface = await pump(tester, null);
    surface.push(
      const DeviceRequest(id: 'd1', cap: 'camera', method: 'take', args: []),
    );
    await tester.pump();
    final result = surface.hostCalls.whereType<DeviceResult>().single;
    expect((result.id, result.code), ('d1', 'unavailable'));
  });

  test('the shell message parses into a DeviceRequest', () {
    final m = HostMessage.parse(
      '{"type":"device","id":"d2","cap":"notify","method":"cancel","args":["water"]}',
    );
    expect(m, isA<DeviceRequest>());
    final r = m! as DeviceRequest;
    expect((r.id, r.cap, r.method), ('d2', 'notify', 'cancel'));
    expect(r.args, ['water']);
    expect(
      HostMessage.parse('{"type":"device","id":"d2","cap":"notify"}'),
      isNull,
    );
    expect(
      const OpenRoute('#/plants').toJs(),
      'window.claxHost?.openRoute("#/plants")',
    );
  });
}
