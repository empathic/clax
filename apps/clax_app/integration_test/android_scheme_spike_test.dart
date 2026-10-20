// The scheme's isolation spike on Android (integration_test/scheme_spike.dart):
// `https://app.clax.invalid` frames `https://abcdefghjkmn.c.clax.invalid`,
// each answered by the runner's SchemeHost
// (android/app/src/main/kotlin/dev/empathic/clax_app/SchemeHost.kt) from a
// Dart table. Run on the emulator "Clax Pixel" with
// `just app-integration --android`.
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
