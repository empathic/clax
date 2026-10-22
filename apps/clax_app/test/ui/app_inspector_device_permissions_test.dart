import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/ui/app_inspector_permissions.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter_test/flutter_test.dart';

import '../platform/device/device_host_test.dart'
    show
        FakeCamera,
        FakeConsent,
        FakeFiles,
        FakeLocation,
        FakeNotifier,
        FakeShare;

void main() {
  InspectedApp app(String id) => InspectedApp(
    id: id,
    title: 'Plants',
    capabilities: const {},
    titles: {id: 'Plants'},
  );

  test('Device lists the allowed and refused capabilities; revoking forgets one and cancels reminders', () async {
    final notifier = FakeNotifier();
    final consent = FakeConsent();
    final device = DeviceHost(
      grants: MemoryDeviceGrants(now: () => DateTime.utc(2026, 10, 12)),
      consent: consent,
      reminders: Reminders(notifier, now: () => DateTime(2026, 10, 12, 9)),
      share: FakeShare(),
      files: FakeFiles(),
      camera: FakeCamera(),
      location: FakeLocation(),
    );
    const aid = 'abcdefghjkmn';
    await device.handle(
      aid,
      const DeviceRequest(
        id: 'a',
        cap: 'notify',
        method: 'schedule',
        args: [
          {
            'id': 'r',
            'title': 'R',
            'every': {'hour': 10, 'minute': 0},
          },
        ],
      ),
    );
    consent.answer = DeviceAnswer.deny;
    await device.handle(
      aid,
      const DeviceRequest(
        id: 'b',
        cap: 'location',
        method: 'current',
        args: [],
      ),
    );

    final source = DevicePermissions(device);
    expect(source.kind, 'device');
    final ps = await source.load(app(aid));
    expect(ps.map((p) => (p.label, p.revokeLabel)), [
      ('Send you notifications and reminders', 'Revoke'),
      ('Not allowed to see where your device is', 'Reset'),
    ]);
    await ps.first.revoke!();
    expect(notifier.scheduled, isEmpty);
    await ps.last.revoke!();
    expect(await source.load(app(aid)), isEmpty);
  });

  test(
    'Device shows how exactly location is allowed, and revokes it',
    () async {
      final consent = FakeConsent()..answer = DeviceAnswer.approximate;
      final device = DeviceHost(
        grants: MemoryDeviceGrants(now: () => DateTime.utc(2026, 10, 12)),
        consent: consent,
        reminders: Reminders(FakeNotifier()),
        share: FakeShare(),
        files: FakeFiles(),
        camera: FakeCamera(),
        location: FakeLocation(),
      );
      const aid = 'abcdefghjkmn';
      await device.handle(
        aid,
        const DeviceRequest(
          id: 'a',
          cap: 'location',
          method: 'current',
          args: [],
        ),
      );
      final source = DevicePermissions(device);
      final ps = await source.load(app(aid));
      expect(
        ps.single.label,
        'See where your device is: approximate, within about 2 km',
      );
      await ps.single.revoke!();
      expect(await source.load(app(aid)), isEmpty);
      consent.answer = DeviceAnswer.allow;
      await device.handle(
        aid,
        const DeviceRequest(
          id: 'b',
          cap: 'location',
          method: 'current',
          args: [],
        ),
      );
      expect(
        (await source.load(app(aid))).single.label,
        'See where your device is: precise',
      );
    },
  );

  test('Network lists the granted hosts and revokes them all', () async {
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Weather', {'index.html': '<h1>W</h1>'});
    final source = NetPermissions(api);
    expect(await source.load(app(aid)), isEmpty);
    api.netDeclared[aid] = ['api.example.com', 'cdn.example.com'];
    api.netGranted[aid] = {
      'api.example.com': '2026-10-12T10:00:00Z',
      'cdn.example.com': '2026-10-12T10:00:00Z',
    };
    final ps = await source.load(app(aid));
    expect(ps.single.label, 'Reaches api.example.com, cdn.example.com');
    await ps.single.revoke!();
    expect(await source.load(app(aid)), isEmpty);
  });

  test(
    'AI shows the spend against the daily cap and revokes the grant',
    () async {
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Coach', {'index.html': '<h1>C</h1>'});
      final source = AiPermissions(api);
      expect(await source.load(app(aid)), isEmpty);
      api.sampleGranted[aid] = '2026-10-12T10:00:00Z';
      api.sampleSpent[aid] = 0.125;
      final ps = await source.load(app(aid));
      expect(ps.single.detail, contains(r'$0.13 of $0.50 spent today'));
      await ps.single.revoke!();
      expect(await source.load(app(aid)), isEmpty);
    },
  );
}
