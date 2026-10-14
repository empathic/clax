import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/signing.dart';
import 'package:flutter_test/flutter_test.dart';

/// A fixed Ed25519 seed for deterministic bundles (RFC 8032 test 1).
final keySeed = Uint8List.fromList(
  hexBytes('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60'),
);

const lineage = 'l_abcdefghijklmnopqrstuvwxyz';

List<int> hexBytes(String hex) => [
  for (var i = 0; i < hex.length; i += 2)
    int.parse(hex.substring(i, i + 2), radix: 16),
];

Uint8List b(String s) => Uint8List.fromList(utf8.encode(s));

const iconSvg =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
    '<rect width="1024" height="1024" fill="#f80"/></svg>';

Map<String, BundleFile> sampleFiles() => {
  'index.html': BundleFile(b('<p>hi</p>'), type: 'text/html'),
  'app.js': BundleFile(b('x'), type: 'text/javascript'),
  'icon.svg': BundleFile(b(iconSvg), type: 'image/svg+xml'),
  'img/dot.png': BundleFile(
    Uint8List.fromList([0x89, 0x50, 0x4e, 0x47, 0, 1, 2, 3, 255]),
    type: 'image/png',
  ),
  'kit/kit.js': BundleFile(b('export const k = 1;\n'), type: 'text/javascript'),
};

Future<WrittenBundle> sampleBundle({
  List<SeedDoc> seed = const [],
  Map<String, Object?>? capabilities,
  Map<String, BundleFile>? files,
}) => BundleWriter.write(
  lineage: lineage,
  version: 3,
  title: 'Plant log',
  iconWord: 'leaf',
  capabilities:
      capabilities ??
      {
        'db': {},
        'net': ['example.com'],
      },
  files: files ?? sampleFiles(),
  seed: seed,
  keySeed: keySeed,
  authorName: 'Alex',
);

/// A valid manifest's JSON for [files] (path to bytes) and [seed].
Map<String, Object?> manifestJson(
  Map<String, Uint8List> files, {
  Map<String, Object?>? seed,
}) => {
  'format': 1,
  'lineage': lineage,
  'version': 1,
  'title': 'T',
  'icon': {'svg': null, 'word': null},
  'capabilities': <String, Object?>{},
  'kit': null,
  'files': {
    for (final MapEntry(:key, :value) in files.entries)
      key: {'sha256': sha256Hex(value), 'size': value.length, 'type': 't'},
  },
  'content_sha256': contentHash({
    for (final MapEntry(:key, :value) in files.entries)
      key: (sha256: sha256Hex(value), size: value.length),
  }),
  'seed': seed,
  'author': {'key': 'A' * 43, 'name': null},
  'forked_from': null,
};

/// A bundle made by hand: [manifest] encoded as given (pretty JSON),
/// signed by [keySeed] (its `author.key` set to match unless [keepKey]),
/// then zipped with [files] under `files/` and [extra] entries after them.
Future<Uint8List> craft(
  Map<String, Object?> manifest,
  Map<String, Uint8List> files, {
  List<ZipEntry> extra = const [],
  Uint8List? seedBytes,
  bool keepKey = false,
  String? signature,
  bool deflate = false,
}) async {
  if (!keepKey) {
    (manifest['author']! as Map)['key'] = await publicKeyFromSeed(keySeed);
  }
  final mbytes = b('${const JsonEncoder.withIndent('  ').convert(manifest)}\n');
  final sig = signature ?? await signManifest(mbytes, keySeed);
  return BundleWriter.zip([
    ZipEntry('manifest.json', mbytes, deflate: deflate),
    ZipEntry('manifest.sig', b(sig)),
    for (final MapEntry(:key, :value) in files.entries)
      ZipEntry('files/$key', value, deflate: deflate),
    if (seedBytes != null) ZipEntry('seed.jsonl', seedBytes),
    ...extra,
  ]);
}

/// Expects [body] to throw a [BundleError] with [code].
Future<void> expectRefused(Future<Object?> Function() body, String code) =>
    expectLater(
      body(),
      throwsA(isA<BundleError>().having((e) => e.code, 'code', code)),
    );

Future<void> expectReadRefused(Uint8List bytes, String code) =>
    expectRefused(() => BundleReader.read(bytes), code);
