import 'dart:io';
import 'dart:math';

import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/platform/bundle_intake.dart';
import 'package:clax_app/platform/bundle_key.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent/loop_support.dart';
import 'bundle/services_support.dart';
import 'support/fake_inspector.dart';
import 'support/fake_supervisor.dart';
import 'support/fake_surface.dart';
import 'support/fake_voice.dart';
import 'ui/bundle_flows_test.dart' show FakeBundleFiles, settle;

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';

  @override
  Future<void> write(String? key) async {}
}

void main() {
  late Directory dir;
  late FakeClaxApi api;
  late FakeSupervisor supervisor;
  late FakeBundleIntake intake;
  late List<FakeAppSurface> surfaces;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('app-bundles-');
    api = FakeClaxApi();
    supervisor = FakeSupervisor();
    intake = FakeBundleIntake();
    surfaces = [];
  });
  tearDown(() => dir.deleteSync(recursive: true));

  AppServices services() => AppServices(
    supervisor: supervisor,
    api: (_) => api,
    backend: (s, k) => HeldBackend([], name: backendMessagesApi),
    voice: FakeVoice.unsupported('No speech here'),
    settings: SettingsStore(File('${dir.path}/settings.json')),
    key: _Key(),
    timings: RecordingTimings(),
    agentTimings: RecordingTimings(),
    surface: (_) {
      final s = FakeAppSurface();
      surfaces.add(s);
      return s;
    },
    installId: 'install',
    supportDir: dir.path,
    prompts: PromptBuilder(rootBundle),
    bundleKey: BundleKey(MemorySeed(), random: Random(3)),
    bundleIntake: intake,
    bundleFiles: FakeBundleFiles(),
    inspector: FakeInspector(),
    pid: 42,
  );

  testWidgets('a file opened before the daemon is up shows its install '
      'sheet once it is, and Install opens the app', (tester) async {
    late String path;
    await tester.runAsync(() async {
      await api.registerAppSession(installId: 'install', pid: 42);
      path = '${dir.path}/in.clax';
      File(path).writeAsBytesSync((await makeBundle(lineage: lineageA)).bytes);
    });
    intake.open(path);
    await tester.pumpWidget(AppRoot(services: services()));
    await tester.pump();
    expect(find.byType(InstallSheet), findsNothing);
    supervisor.set(Up(endpoint()));
    await settle(tester);
    expect(find.byType(InstallSheet), findsOneWidget);
    expect(
      File(path).existsSync(),
      isFalse,
      reason: 'tests run as Android, where the file is Clax’s own copy',
    );
    await tester.tap(find.text('Install'));
    // The opened app's page shows an endless progress indicator until its
    // shell is ready, so pump rather than settle.
    for (var i = 0; i < 20; i++) {
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(find.byType(InstallSheet), findsNothing);
    expect(surfaces, isNotEmpty, reason: 'the installed app opened');
  });

  testWidgets('a platform refusal shows its sentence', (tester) async {
    await tester.pumpWidget(AppRoot(services: services()));
    supervisor.set(Up(endpoint()));
    await settle(tester);
    intake.refuse('too_large');
    await settle(tester);
    expect(find.text(refusalMessage('too_large')), findsOneWidget);
  });

  testWidgets('a deleted app’s lineage is forgotten', (tester) async {
    final store = LineageStore(dir);
    await tester.runAsync(
      () => store.put(
        'a_gone',
        const LineageEntry(
          lineage: lineageA,
          role: LineageRole.installed,
          key: 'k',
          name: null,
          version: 1,
          released: [],
          capabilities: {},
          title: 'Gone',
        ),
      ),
    );
    await tester.pumpWidget(AppRoot(services: services()));
    supervisor.set(Up(endpoint()));
    await settle(tester);
    api.emit(
      const ClaxEvent(
        type: 'artifact_deleted',
        data: {'artifact_id': 'a_gone'},
      ),
    );
    await settle(tester);
    await tester.runAsync(() async {
      expect(await store.entry('a_gone'), isNull);
    });
  });

  testWidgets('an update kept at start asks Restore or Discard; Discard '
      'keeps the documents and drops the snapshot', (tester) async {
    final snapshots = DocSnapshots(api, dir);
    late String aid;
    await tester.runAsync(() async {
      await api.registerAppSession(installId: 'install', pid: 42);
      aid = (await api.createArtifact(
        PublishRequest(title: 'Plant log', files: {'index.html': '<p>v1</p>'}),
      )).artifact.id;
      api.putDoc(aid, 'plants/p1', {'name': 'fern'});
      await snapshots.take(
        aid,
        const SnapshotMeta(
          base: 1,
          contentSha256: 'x',
          title: 'Plant log',
          backOn: 1,
        ),
      );
      snapshots.writeMeta(
        aid,
        snapshots.meta(aid)!.copyWith(published: 2, failed: true),
      );
      api.putDoc(aid, 'plants/p2', {'name': 'written later'});
    });
    await tester.pumpWidget(AppRoot(services: services()));
    supervisor.set(Up(endpoint()));
    await settle(tester);
    expect(find.byType(KeptUpdateSheet), findsOneWidget);
    await tester.tap(find.text('Discard'));
    await settle(tester);
    await tester.runAsync(() async {
      expect(snapshots.has(aid), isFalse);
      expect(await api.getDoc(aid, 'plants/p2'), isNotNull);
    });
  });
}
