// The home app bundle the app ships (assets/home/home.clax, built from
// home/ by //apps/clax_app:home_bundle, or natively by
// scripts/build-home-bundle.sh): it reads and verifies as any bundle,
// carries what home/manifest.json says, matches its release record
// (assets/home/release.json), and holds today's home/ sources and the kit
// files the manifest names, byte for byte.
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:flutter_test/flutter_test.dart';
import '../support/paths.dart';

void main() {
  final manifest = jsonDecode(
    File(appPath('home/manifest.json')).readAsStringSync(),
  ) as Map<String, Object?>;
  final release = jsonDecode(
    File(appPath('assets/home/release.json')).readAsStringSync(),
  ) as Map<String, Object?>;
  final kit = (manifest['kit'] as List).cast<String>();

  Future<Bundle> shipped() async =>
      BundleReader.read(File(appPath('assets/home/home.clax')).readAsBytesSync());

  test(
    'home.clax verifies and carries the manifest it was packed from',
    () async {
      final m = (await shipped()).manifest;
      expect(m.title, manifest['title']);
      expect(m.icon.word, manifest['icon']);
      expect(m.capabilities, manifest['capabilities']);
      expect(m.capabilities.keys, containsAll(['home', 'db']));
      expect(m.version, greaterThan(manifest['version'] as int));
      expect(release['version'], m.version);
      expect(release['release'], '${manifest['release']}.${m.version}');
      expect(release['lineage'], m.lineage);
      expect(release['hash'], m.contentSha256);
      expect(release['key'], m.author.key);
    },
  );

  test('home.clax is the release, under the home app\'s key and lineage, '
      'or a development build, on the development lineage', () async {
    final m = (await shipped()).manifest;
    if (m.author.key == manifest['key']) {
      expect(m.lineage, manifest['lineage']);
    } else {
      expect(
        m.lineage,
        manifest['dev_lineage'],
        reason: 'a bundle under another key than home/manifest.json\'s '
            'must not take the home app\'s lineage',
      );
    }
  });

  test('home.clax holds the current home/ sources and the kit files the '
      'manifest names', () async {
    final b = await shipped();
    final want = <String, List<int>>{};
    final source = Directory(appPath('home'));
    for (final e in source.listSync(recursive: true)) {
      if (e is! File) continue;
      final path = e.path.substring(source.path.length + 1);
      if (path == 'manifest.json' ||
          path.split('/').any((p) => p.startsWith('.'))) {
        continue;
      }
      want[path] = e.readAsBytesSync();
    }
    for (final n in kit) {
      want['kit/$n'] = File(appPath('assets/kit/$n')).readAsBytesSync();
    }
    expect(b.files.keys.toSet(), want.keys.toSet());
    for (final MapEntry(:key, :value) in want.entries) {
      expect(b.files[key], value, reason: '$key differs from its source');
    }
  });
}
