import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/exporter.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/manifest.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/bundle_key.dart';

import '../support/fake_inspector.dart';

/// A [BundleSeedStorage] in memory.
class MemorySeed implements BundleSeedStorage {
  String? seed;

  @override
  Future<String?> read() async => seed;

  @override
  Future<void> write(String s) async => seed = s;
}

Uint8List utf(String s) => Uint8List.fromList(utf8.encode(s));

/// One Clax install: a daemon (fake), a support directory, a key and the
/// services over them.
class Install {
  Install(this.dir, {int keySeed = 1})
    : api = FakeClaxApi(),
      inspector = FakeInspector() {
    store = LineageStore(Directory('${dir.path}/support'));
    key = BundleKey(MemorySeed(), random: Random(keySeed));
    snapshots = DocSnapshots(api, Directory('${dir.path}/support'));
    exporter = Exporter(api: api, store: store, key: key, random: Random(7));
    installer = Installer(api: api, store: store, key: key);
    updater = Updater(
      api: api,
      store: store,
      inspector: inspector,
      baseUrl: () => Uri.parse('http://127.0.0.1:9/'),
      snapshots: snapshots,
      busy: (aid) => busy.contains(aid),
    );
  }

  final Directory dir;
  final FakeClaxApi api;
  final FakeInspector inspector;
  late final LineageStore store;
  late final BundleKey key;
  late final DocSnapshots snapshots;
  late final Exporter exporter;
  late final Installer installer;
  late final Updater updater;
  final Set<String> busy = {};

  Future<void> ready() =>
      api.registerAppSession(installId: 'i-${dir.path.hashCode}', pid: 1);
}

/// A bundle of [files] signed by [keySeed] (32 bytes from [seedByte]).
Future<WrittenBundle> makeBundle({
  required String lineage,
  int version = 1,
  String title = 'Plant log',
  Map<String, String> files = const {'index.html': '<p>v1</p>'},
  Map<String, Object?> capabilities = const {'db': {}},
  List<SeedDoc> seed = const [],
  int seedByte = 9,
  String? name = 'Sam',
}) => BundleWriter.write(
  lineage: lineage,
  version: version,
  title: title,
  iconWord: 'leaf',
  capabilities: capabilities,
  files: {
    for (final MapEntry(:key, :value) in files.entries)
      key: BundleFile(utf(value), type: _type(key)),
  },
  seed: seed,
  keySeed: List.filled(32, seedByte),
  authorName: name,
);

String _type(String path) => path.endsWith('.html')
    ? 'text/html; charset=utf-8'
    : path.endsWith('.js')
    ? 'text/javascript; charset=utf-8'
    : 'text/plain; charset=utf-8';

const lineageA = 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa';
