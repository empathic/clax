// Renders the first frame of a cold start that has a home screen snapshot:
// the app before its daemon is up, showing the snapshot kept in a
// home-snapshot.bin (as a run of the app wrote it), for a person to look at.
// Off by default; run with
//   CLAX_HOME_SHOTS=1 CLAX_HOME_SNAPSHOT=<cache>/home/home-snapshot.bin \
//     flutter test --update-goldens test/home/home_snapshot_shots_test.dart
// and look at target/flutter-app/home/launch-snapshot.png.
import 'dart:io';

import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/home/home_snapshot.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/loop_support.dart';
import '../support/fake_supervisor.dart';
import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

final _on = Platform.environment['CLAX_HOME_SHOTS'] == '1';
final _file = Platform.environment['CLAX_HOME_SNAPSHOT'] ?? '';
// Goldens resolve from this file.
const _out = '../../../../target/flutter-app/home';

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';
  @override
  Future<void> write(String? key) async {}
}

void main() {
  testWidgets(
    'the first frame of a cold start with a snapshot',
    skip: !_on || _file.isEmpty,
    (tester) async {
      final dir = Directory.systemTemp.createTempSync('snapshot-shots-');
      addTearDown(() => dir.deleteSync(recursive: true));
      final source = File(_file);
      final snapshot = await tester.runAsync(
        () => HomeSnapshotStore(source.parent).load(),
      );
      expect(snapshot, isNotNull, reason: '$_file holds a snapshot');
      await tester.runAsync(
        () => (HomeSettings.loadSync(dir)..homeAppId = snapshot!.aid).save(),
      );
      tester.view.physicalSize = Size(snapshot!.width * 2, snapshot.height * 2);
      tester.view.devicePixelRatio = 2;
      addTearDown(tester.view.reset);
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      await tester.pumpWidget(
        AppRoot(
          services: AppServices(
            supervisor: FakeSupervisor(),
            api: (_) => FakeClaxApi(),
            backend: (s, k) => HeldBackend([], name: backendMessagesApi),
            voice: FakeVoice.unsupported('No speech here'),
            settings: SettingsStore(File('${dir.path}/settings.json')),
            key: _Key(),
            timings: RecordingTimings(),
            agentTimings: RecordingTimings(),
            surface: (_) => FakeAppSurface(),
            installId: 'install',
            supportDir: dir.path,
            prompts: PromptBuilder(rootBundle),
            homeSnapshot: snapshot,
            pid: 42,
          ),
        ),
      );
      expect(find.byKey(const ValueKey('launch-snapshot')), findsOneWidget);
      await expectLater(
        find.byType(AppRoot),
        matchesGoldenFile('$_out/launch-snapshot.png'),
      );
      snapshot.image?.dispose();
    },
  );
}
