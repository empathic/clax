import 'dart:io';
import 'dart:math';

import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/platform/bundle_intake.dart';
import 'package:clax_app/platform/bundle_key.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/puller.dart' show notAnAppLink;
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent/loop_support.dart';
import 'bundle/services_support.dart';
import 'support/fake_inspector.dart';
import 'support/fake_core.dart';
import 'support/fake_surface.dart';
import 'support/fake_voice.dart';
import 'support/settle.dart';
import 'ui/bundle_flows_test.dart' show FakeBundleFiles;

/// A [FakeRemoteApi] that keeps the addresses of the anonymous heads it
/// was asked for.
class _Remote extends FakeRemoteApi {
  final heads = <Uri>[];

  @override
  Future<RemoteHead> anonymousHead(Uri artifactUrl) {
    heads.add(artifactUrl);
    return super.anonymousHead(artifactUrl);
  }
}

/// The home screen, there once the core is up.
final _home = find.byKey(const ValueKey('home'));

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';

  @override
  Future<void> write(String? key) async {}
}

void main() {
  late Directory dir;
  late FakeClaxApi api;
  late FakeCoreHost coreHost;
  late FakeBundleIntake intake;
  late List<FakeAppSurface> surfaces;
  late _Remote remote;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('app-bundles-');
    api = FakeClaxApi();
    coreHost = FakeCoreHost();
    intake = FakeBundleIntake();
    surfaces = [];
    remote = _Remote();
  });
  tearDown(() => dir.deleteSync(recursive: true));

  AppServices services() => AppServices(
    core: coreHost,
    api: (_) => api,
    backend: (s, k) => HeldBackend([], name: backendMessagesApi),
    voice: FakeVoice.unsupported('No speech here'),
    settings: SettingsStore(File('${dir.path}/settings.json')),
    key: _Key(),
    timings: RecordingTimings(),
    agentTimings: RecordingTimings(),
    surface: () {
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
    remote: () => remote,
    pid: 42,
  );

  testWidgets('a file opened at launch shows its install sheet as the core '
      'opens, and Install opens the app', (tester) async {
    late String path;
    await tester.runAsync(() async {
      await api.registerAppSession(installId: 'install', pid: 42);
      path = '${dir.path}/in.clax';
      File(path).writeAsBytesSync((await makeBundle(lineage: lineageA)).bytes);
    });
    intake.open(path);
    await tester.pumpWidget(AppRoot(services: services()));
    await tester.pump();
    coreHost.set(const CoreReady());
    await settle(tester, find.byType(InstallSheet));
    expect(find.byType(InstallSheet), findsOneWidget);
    expect(
      File(path).existsSync(),
      isFalse,
      reason: 'tests run as Android, where the file is Clax’s own copy',
    );
    await tester.tap(find.text('Install'));
    await settleUntil(
      tester,
      () => surfaces.isNotEmpty && find.byType(InstallSheet).evaluate().isEmpty,
    );
    expect(find.byType(InstallSheet), findsNothing);
    expect(surfaces, isNotEmpty, reason: 'the installed app opened');
  });

  for (final (platform, ownCopy) in [
    (TargetPlatform.iOS, true),
    (TargetPlatform.macOS, false),
  ]) {
    testWidgets('on ${platform.name} the opened file is '
        '${ownCopy ? 'Clax’s own copy, deleted once read' : 'the person’s, kept'}', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      late String path;
      await tester.runAsync(() async {
        await api.registerAppSession(installId: 'install', pid: 42);
        path = '${dir.path}/in.clax';
        File(path).writeAsBytesSync((await makeBundle(lineage: lineageA)).bytes);
      });
      intake.open(path);
      await tester.pumpWidget(AppRoot(services: services()));
      coreHost.set(const CoreReady());
      await settle(tester, find.byType(InstallSheet));
      expect(find.byType(InstallSheet), findsOneWidget);
      expect(File(path).existsSync(), !ownCopy);
      debugDefaultTargetPlatformOverride = null;
    });
  }

  testWidgets('a platform refusal shows its sentence', (tester) async {
    await tester.pumpWidget(AppRoot(services: services()));
    coreHost.set(const CoreReady());
    await settle(tester, _home);
    intake.refuse('too_large');
    await settle(tester, find.text(refusalMessage('too_large')));
    expect(find.text(refusalMessage('too_large')), findsOneWidget);
  });

  testWidgets('url_event_pulls', (tester) async {
    await tester.pumpWidget(AppRoot(services: services()));
    coreHost.set(const CoreReady());
    await settle(tester, _home);
    intake.openLink('https://clax.dev/a/7q3k9mzx2b4t/get');
    await settle(tester, find.text('Couldn’t get this app'));
    expect(remote.heads, [Uri.parse('https://clax.dev/a/7q3k9mzx2b4t')]);
    // Nothing is published there in the fake, so the get says so.
    expect(find.text('Couldn’t get this app'), findsOneWidget);
  });

  testWidgets('a refused link is titled for a link', (tester) async {
    await tester.pumpWidget(AppRoot(services: services()));
    coreHost.set(const CoreReady());
    await settle(tester, _home);
    intake.openLink('https://example.com/a/7q3k9mzx2b4t/get');
    await settle(tester, find.text('Couldn’t open this link'));
    expect(find.text('Couldn’t open this link'), findsOneWidget);
    expect(find.text('Couldn’t open this file'), findsNothing);
    expect(find.text(notAnAppLink), findsOneWidget);
    expect(remote.heads, isEmpty);
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
    coreHost.set(const CoreReady());
    await settle(tester, _home);
    api.emit('artifact_deleted', {'artifact_id': 'a_gone'});
    await settleUntil(tester, () async => await store.entry('a_gone') == null);
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
    coreHost.set(const CoreReady());
    await settle(tester, find.byType(KeptUpdateSheet));
    expect(find.byType(KeptUpdateSheet), findsOneWidget);
    await tester.tap(find.text('Discard'));
    await settleUntil(tester, () => !snapshots.has(aid));
    await tester.runAsync(() async {
      expect(snapshots.has(aid), isFalse);
      expect(await api.getDoc(aid, 'plants/p2'), isNotNull);
    });
  });
}
