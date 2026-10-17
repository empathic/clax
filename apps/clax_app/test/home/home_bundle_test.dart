// The shipped home app bundle (assets/home/home.clax, packed from home/ by
// scripts/build-home-bundle.sh): it reads and verifies as any bundle,
// carries what home/manifest.json says, matches assets/home/release.json,
// and holds today's home/ sources, so a change to them without packing
// again fails here.
//
// The bundle carries its own copy of the kit files the manifest names, as
// they were at its last packing, and may lag assets/kit/: a kit edit does
// not fail here (only the holder of the home signing key can pack again;
// see docs/usage.md, "The home app"). The kit drift check below reports
// the files that differ, as a skipped test with its reason.
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
    final own = {
      for (final p in b.files.keys)
        if (!p.startsWith('kit/')) p,
    };
    expect(own, want.keys.toSet(), reason: 'run just home-bundle');
    expect(
      {
        for (final p in b.files.keys)
          if (p.startsWith('kit/')) p,
      },
      {for (final n in kit) 'kit/$n'},
      reason: 'run just home-bundle',
    );
    for (final MapEntry(:key, :value) in want.entries) {
      expect(b.files[key], value, reason: '$key differs; run just home-bundle');
    }
  });

  test('kit drift: the kit files home.clax carries against assets/kit/ '
      '(reported, never failing)', () async {
    final b = await shipped();
    final drifted = [
      for (final n in kit)
        if (!_same(b.files['kit/$n'], File(appPath('assets/kit/$n')).readAsBytesSync()))
          n,
    ];
    if (drifted.isNotEmpty) {
      markTestSkipped(
        'home.clax carries older copies of assets/kit/ '
        '${drifted.join(', ')}; the home app keeps working with them. The '
        'holder of the home signing key picks up the new kit by raising '
        'version and release in home/manifest.json and running just '
        'home-bundle.',
      );
    }
  });
}

bool _same(List<int>? a, List<int> b) {
  if (a == null || a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}
