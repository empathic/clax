import 'dart:io';
import 'dart:math';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/exporter.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/platform/bundle_key.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import '../support/real_core.dart';

/// An app that reads another app (`db.read`) exported, installed into a
/// second home and updated there, over two real embedded cores. The
/// daemon writes each version's `clax.json` from its declaration, and that
/// file names the other app: a bundle carries none, and the installer and
/// updater pass the bundle's declaration as `capabilities`, so the daemon
/// writes the file again on the other home.
void main() {
  test(
    'an app declaring db.read exports, installs and updates, its bundle holding no clax.json',
    () async {
      final a = await _Home.open('A', seed: 1);
      final b = await _Home.open('B', seed: 2);

      final runs = await a.api.createArtifact(
        PublishRequest(
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': '<p>runs</p>'},
        ),
      );
      final created = await a.api.createArtifact(
        PublishRequest(
          title: 'Dashboard',
          capabilities: {
            'db': {
              'read': [runs.artifact.id],
            },
          },
          files: {'index.html': '<p>one</p>'},
        ),
      );
      final aid = created.artifact.id;
      expect(created.version.files.keys, contains('clax.json'));

      final first = await a.exporter.export(aid);
      final v1 = await BundleReader.read(first.bytes);
      expect(v1.files.keys, isNot(contains('clax.json')));
      expect(v1.manifest.capabilities, {'db': <String, Object?>{}});

      final mine = (await b.installer.install(
        await b.installer.open(first.bytes),
      )).aid;
      final installed = await b.api.getArtifact(mine);
      expect(installed.artifact.capabilities, {'db': <String, Object?>{}});
      expect(installed.current?.files.keys, contains('clax.json'));
      expect(installed.current?.contentSha256, v1.manifest.contentSha256);

      await a.api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>two</p>'}),
      );
      final second = await a.exporter.export(aid);
      final v2 = await BundleReader.read(second.bytes);
      expect(v2.files.keys, isNot(contains('clax.json')));

      final out = await b.updater.update(mine, v2);
      expect(out, isA<UpdateLanded>(), reason: '$out');
      final updated = await b.api.getArtifact(mine);
      expect(updated.artifact.currentVersion, 2);
      expect(updated.artifact.capabilities, {'db': <String, Object?>{}});
      expect(
        await b.api.readFiles(mine, 2),
        containsPair('index.html', '<p>two</p>'),
      );
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

/// One app home over a real core: its API, lineage store, install key and
/// the bundle services the app runs.
class _Home {
  _Home._(this.api, this.exporter, this.installer, this.updater);

  final HttpClaxApi api;
  final Exporter exporter;
  final Installer installer;
  final Updater updater;

  static Future<_Home> open(String name, {required int seed}) async {
    final core = await RealCore.open();
    final api = core.api();
    await api.registerAppSession(
      installId: 'install-$name',
      pid: pid,
      cwd: core.dir.path,
    );
    final support = Directory('${core.dir.path}/support');
    final store = LineageStore(support);
    final key = BundleKey(_Seed(), random: Random(seed));
    return _Home._(
      api,
      Exporter(api: api, store: store, key: key, random: Random(seed)),
      Installer(api: api, store: store, key: key),
      Updater(
        api: api,
        store: store,
        inspector: FakeInspector(),
        baseUrl: () => core.shell,
        snapshots: DocSnapshots(api, support),
        busy: (_) => false,
      ),
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
