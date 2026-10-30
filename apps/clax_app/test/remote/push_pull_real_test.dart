import 'dart:io';
import 'dart:math';

import 'package:clax_app/bundle/exporter.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/platform/bundle_key.dart';
import 'package:clax_app/remote/checker.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/remote/remote_api.dart';
import 'package:clax_app/remote/sync_state.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_clock.dart';
import '../support/hosted_daemon.dart';
import '../support/real_core.dart';

/// Publishing, pulling and updating an app through a hosted Clax, with the
/// app's own services over two real embedded cores (two homes) signed in
/// to one hosted daemon: one home publishes, the other pulls, the first
/// republishes, and the second's checker offers the update.
void main() {
  test(
    'publish, pull into a second home, republish, and the second is offered the update',
    () async {
      final hosted = await HostedDaemon.start();
      final clock = FakeClock();
      final a = await _Home.open(hosted, 'A', seed: 1);
      final b = await _Home.open(hosted, 'B', seed: 2);

      // A makes an app and publishes its first version.
      final created = await a.api.createArtifact(
        PublishRequest(
          title: 'Plant log',
          files: {'index.html': '<h1>one</h1>'},
        ),
      );
      final aid = created.artifact.id;
      final publisher = Publisher(
        remote: a.remote,
        exporter: Exporter(api: a.api, store: a.store, key: a.key),
        store: a.store,
        clock: clock,
        authorName: () async => 'Ada',
      );
      final first = await publisher.publish(aid, version: 1);
      expect(first, isA<Published>());
      final url = (first as Published).url;
      expect(first.remoteVersion, 1);
      expect(Uri.parse(url).origin, hosted.origin);

      // B pulls it into its own home.
      final puller = Puller(
        remote: b.remote,
        installer: Installer(api: b.api, store: b.store, key: b.key),
        store: b.store,
        installId: () async => 'install-b',
      );
      final pulled = await puller.pull(Uri.parse(url));
      expect(pulled, isA<PullInstalled>(), reason: '$pulled');
      final mine = (pulled as PullInstalled).aid;
      final installed = await b.api.getArtifact(mine);
      final v1 = await a.api.getArtifact(aid);
      expect(installed.current?.contentSha256, v1.current?.contentSha256);
      final entry = await b.store.entry(mine);
      expect(entry?.link?.baseRemoteVersion, 1);

      final checker = LinkedChecker(
        api: b.remote,
        store: b.store,
        clock: clock,
        live: liveFrom(b.api),
      );
      await checker.checkNow(mine);
      expect(checker.status.value[mine]?.state, isNot(SyncState.updateWaiting));

      // A republishes: a second version, public version 2 with its bundle.
      await a.api.publish(
        aid,
        PublishRequest(
          ifVersion: 1,
          files: {'index.html': '<h1>two</h1>'},
        ),
      );
      final second = await publisher.publish(aid, version: 2);
      expect(second, isA<Published>(), reason: '$second');
      expect((second as Published).remoteVersion, 2);

      // B's checker reads the new head and offers it.
      await checker.checkNow(mine);
      final status = checker.status.value[mine];
      expect(status?.state, SyncState.updateWaiting);
      expect(status?.head?.version, 2);
    },
    skip: switch ((RealCore.skip, HostedDaemon.skip)) {
      (final String why, _) || (_, final String why) => why,
      _ => false,
    },
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

/// One app home: a real core on a scratch home, the app's API and remote
/// API over it, its lineage store and its install key, signed in to
/// [HostedDaemon] as the app signs in (device code, kind `clax-app`), the
/// owner approving.
class _Home {
  _Home._(this.api, this.remote, this.store, this.key);

  final HttpClaxApi api;
  final RemoteApi remote;
  final LineageStore store;
  final BundleKey key;

  static Future<_Home> open(
    HostedDaemon hosted,
    String name, {
    required int seed,
  }) async {
    final core = await RealCore.open();
    final api = core.api();
    await api.registerAppSession(
      installId: 'install-$name',
      pid: pid,
      cwd: core.dir.path,
    );
    final remote = HttpRemoteApi(CoreTransport(core.host.calls));
    final signIn = await remote.addRemote(
      hosted.origin,
      machine: 'Clax on $name',
    );
    await hosted.approve(signIn.userCode);
    final r = await remote.poll(signIn.remote.id);
    expect(r.connected, isTrue, reason: 'status ${r.status}');
    return _Home._(
      api,
      remote,
      LineageStore(Directory('${core.dir.path}/support')),
      BundleKey(_Seed(), random: Random(seed)),
    );
  }
}

/// A [BundleSeedStorage] in memory.
class _Seed implements BundleSeedStorage {
  String? seed;

  @override
  Future<String?> read() async => seed;

  @override
  Future<void> write(String s) async => seed = s;
}
