// The scheme's isolation spike on the Mac (integration_test/scheme_spike.dart):
// `clax://app` frames `clax://abcdefghjkmn.c`, each answered by the runner's
// SchemeHost (macos/Runner/SchemeHost.swift) from a Dart table. Run with
// `just app-integration` (a debug build).
import 'package:clax_app/core/origins.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'scheme_spike.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a content frame is its own origin and reaches nothing of the shell', (
    t,
  ) async {
    final origins = AppOrigins.forPlatform();
    final report = await runSpike(t, origins);
    printSpikeReport(report);
    expectIsolated(report, origins);
  });
}
