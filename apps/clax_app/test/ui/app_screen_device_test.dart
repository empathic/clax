import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/material.dart';
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
      expect(result.value, {'lat': 52.5, 'lon': 13.4, 'accuracy': 12.0});
      expect(consent.asked.single.$2, 'location');
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
