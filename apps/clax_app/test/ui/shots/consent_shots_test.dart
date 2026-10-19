// The consent dialogs rendered in every form and theme into CLAX_SHOTS_DIR
// (skipped without it): Clax asking to read another app, and an app asking
// for the device's location.
import 'package:clax_app/agent/read_consent.dart';
import 'package:clax_app/platform/device/device_consent_dialog.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:flutter/material.dart';

import 'rig.dart';

/// Shows a page, runs [ask] with its navigator, and takes the shot.
Future<void> _over(
  Shot s,
  String surface,
  void Function(NavigatorState nav) ask,
) async {
  final nav = GlobalKey<NavigatorState>();
  await s.tester.pumpWidget(
    s.app(
      Navigator(
        key: nav,
        onGenerateRoute: (_) => MaterialPageRoute<void>(
          builder: (_) => Scaffold(
            appBar: AppBar(title: const Text('Dashboard')),
            body: const Center(child: Text('A page')),
          ),
        ),
      ),
    ),
  );
  ask(nav.currentState!);
  await s.settle();
  await s.take(surface);
}

void main() {
  shots(
    'consent-read',
    (s) => _over(s, 'consent-read', (nav) {
      Future<ConsentAnswer> _ = DialogReadConsent(
        navigator: () => nav,
        memory: MemoryConsentMemory(),
      ).ask(
        reader: 'a0000000000a',
        readerTitle: 'Dashboard',
        source: 'b0000000000b',
        sourceTitle: 'Groceries',
      );
    }),
  );
  shots(
    'consent-location',
    (s) => _over(s, 'consent-location', (nav) {
      Future<DeviceAnswer> _ = DialogDeviceConsent(
        navigator: () => nav,
        title: (_) async => 'Plant care',
      ).ask('abcdefghjkmn', 'location');
    }),
  );
}
