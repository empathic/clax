// The scheme's isolation spike on the simulator "Clax iPhone"
// (integration_test/scheme_spike.dart): `clax://app` frames
// `clax://abcdefghjkmn.c`, each answered by the runner's SchemeHost
// (ios/Runner/SchemeHost.swift, the Mac's file) from a Dart table.
//
// Run: `bazelisk test //apps/clax_app:integration_ios_scheme_spike`, or
// `just app-integration --ios` (tool/ios-integration-run.sh).
import 'package:clax_app/core/origins.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'scheme_spike.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'a content frame is its own origin and reaches nothing of the shell',
    (t) async {
      final origins = AppOrigins.forPlatform();
      final report = await runSpike(t, origins);
      printSpikeReport(report);
      // The iOS rig's end (integration_test/ios_rig.dart): unmount, and turn
      // off the semantics the simulator turns on when a host accessibility
      // client reads it, whose handle the framework would count as left open.
      await t.pumpWidget(const SizedBox());
      final dispatcher = t.binding.platformDispatcher;
      if (dispatcher.semanticsEnabled) {
        dispatcher.semanticsEnabledTestValue = false;
      }
      expectIsolated(report, origins);
    },
  );
}
