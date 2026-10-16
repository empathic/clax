// The shipped home app bundle (assets/home/home.clax, packed from home/ by
// scripts/build-home-bundle.sh): it reads and verifies as any bundle,
// carries what home/manifest.json says, matches assets/home/release.json,
// and holds today's sources, so a change to them without packing again
// fails here.
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final manifest = jsonDecode(
    File('home/manifest.json').readAsStringSync(),
  ) as Map<String, Object?>;
  final release = jsonDecode(
    File('assets/home/release.json').readAsStringSync(),
  ) as Map<String, Object?>;

  test(
    'home.clax verifies and carries the manifest it was packed from',
    () async {
      final b = await BundleReader.read(
        File('assets/home/home.clax').readAsBytesSync(),
      );
      final m = b.manifest;
      expect(m.title, 'Home');
      expect(m.icon.word, manifest['icon']);
      expect(m.capabilities, manifest['capabilities']);
      expect(m.capabilities.keys, containsAll(['home', 'db']));
      expect(m.lineage, manifest['lineage']);
      expect(m.version, manifest['version']);
      expect(release['release'], manifest['release']);
      expect(release['version'], m.version);
      expect(release['lineage'], m.lineage);
      expect(release['hash'], m.contentSha256);
      expect(release['key'], m.author.key);
    },
  );

  test('home.clax holds the current home/ sources and kit files', () async {
    final b = await BundleReader.read(
      File('assets/home/home.clax').readAsBytesSync(),
    );
    final want = <String, List<int>>{};
    final source = Directory('home');
    for (final e in source.listSync(recursive: true)) {
      if (e is! File) continue;
      final path = e.path.substring(source.path.length + 1);
      if (path == 'manifest.json' ||
          path.split('/').any((p) => p.startsWith('.'))) {
        continue;
      }
      want[path] = e.readAsBytesSync();
    }
    for (final name in (manifest['kit'] as List).cast<String>()) {
      want['kit/$name'] = File('assets/kit/$name').readAsBytesSync();
    }
    expect(b.files.keys.toSet(), want.keys.toSet());
    for (final MapEntry(:key, :value) in want.entries) {
      expect(b.files[key], value, reason: '$key differs; run just home-bundle');
    }
  });
}
