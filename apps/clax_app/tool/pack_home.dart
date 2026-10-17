// Packs the home app's source (`home/`) into the bundle the Clax app ships
// (`assets/home/home.clax`) and its release record (`assets/home/
// release.json`). Run through scripts/build-home-bundle.sh, which names the
// signing key and passes `--write` only when asked to act.
//
//   dart run tool/pack_home.dart --key <seed file> [--write] [--app <dir>]
//
// `--app` names apps/clax_app; the default is this script's parent
// directory's parent (a compiled pack_home, as Bazel builds it, is run
// with it).
//
// `home/manifest.json` gives the bundle's title, icon word, capabilities,
// lineage, version and release, and the kit files (from `assets/kit/`) the
// bundle carries under `kit/`; every other file under `home/` is carried
// as it is. The bundle is deterministic (BundleWriter), so packing
// unchanged sources gives the same bytes and nothing is written.
//
// The bundle is signed by the home app's own Ed25519 key, kept outside the
// repository: the seed file holds its 32 bytes in base64url without
// padding. `release.json` records the key's public half, so every later
// release must be signed by the same key (the trust rules, lib/bundle/
// trust.dart, treat another key as a foreign lineage): when that record
// exists and the seed file does not, or holds another key, packing stops.
// With no record yet, `--write` makes the key (Random.secure) and saves it.
//
// A release keeps its version only while its content is unchanged: packing
// changed files under the recorded version stops, and asks for a higher
// `version` and `release` in `home/manifest.json`.
//
// Exits 0 when the outputs are current or were written, 1 when packing
// stops, 2 on a usage error.
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/signing.dart';

const _types = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.txt': 'text/plain',
  '.png': 'image/png',
};

Never _stop(String why, [int code = 1]) {
  stderr.writeln('pack_home: $why');
  exit(code);
}

String _type(String path) {
  final dot = path.lastIndexOf('.');
  final t = dot < 0 ? null : _types[path.substring(dot).toLowerCase()];
  return t ?? _stop('$path: no content type for its extension');
}

/// Every file under [dir], by its `/`-separated path relative to [dir].
Map<String, File> _tree(Directory dir) => {
  for (final e in dir.listSync(recursive: true, followLinks: false))
    if (e is File)
      e.path.substring(dir.path.length + 1).replaceAll(r'\', '/'): e,
};

Future<void> main(List<String> args) async {
  String? keyPath;
  String? appPath;
  var write = false;
  for (var i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--write':
        write = true;
      case '--key' when i + 1 < args.length:
        keyPath = args[++i];
      case '--app' when i + 1 < args.length:
        appPath = args[++i];
      default:
        _stop(
          'usage: dart run tool/pack_home.dart --key <seed file> [--write] '
          '[--app <dir>]',
          2,
        );
    }
  }
  if (keyPath == null) _stop('--key is required', 2);

  final app = appPath != null
      ? Directory(appPath)
      : File(Platform.script.toFilePath()).parent.parent;
  final source = Directory('${app.path}/home');
  final kit = Directory('${app.path}/assets/kit');
  final out = Directory('${app.path}/assets/home');
  final bundleFile = File('${out.path}/home.clax');
  final releaseFile = File('${out.path}/release.json');

  final m = jsonDecode(
    File('${source.path}/manifest.json').readAsStringSync(),
  ) as Map<String, Object?>;
  final title = m['title'] as String;
  final iconWord = m['icon'] as String?;
  final capabilities = (m['capabilities'] as Map).cast<String, Object?>();
  final lineage = m['lineage'] as String;
  final version = m['version'] as int;
  final release = m['release'] as String;
  final kitFiles = (m['kit'] as List).cast<String>();

  final files = <String, BundleFile>{};
  for (final MapEntry(key: path, value: f) in _tree(source).entries) {
    if (path == 'manifest.json' ||
        path.split('/').any((p) => p.startsWith('.'))) {
      continue;
    }
    files[path] = BundleFile(f.readAsBytesSync(), type: _type(path));
  }
  for (final name in kitFiles) {
    final f = File('${kit.path}/$name');
    if (!f.existsSync()) {
      _stop('assets/kit/$name, named in home/manifest.json, is missing');
    }
    files['kit/$name'] = BundleFile(f.readAsBytesSync(), type: _type(name));
  }
  final hash = contentHash({
    for (final MapEntry(:key, :value) in files.entries)
      key: (sha256: sha256Hex(value.bytes), size: value.bytes.length),
  });

  final recorded = releaseFile.existsSync()
      ? jsonDecode(releaseFile.readAsStringSync()) as Map<String, Object?>
      : null;
  if (recorded != null &&
      recorded['version'] == version &&
      recorded['hash'] != hash) {
    _stop(
      'the files changed since release ${recorded['release']} (version '
      '$version); raise "version" and "release" in home/manifest.json',
    );
  }
  if (recorded != null && (recorded['version'] as int) > version) {
    _stop(
      'home/manifest.json version $version is below the shipped ${recorded['version']}',
    );
  }

  final keyFile = File(keyPath);
  Uint8List seed;
  if (keyFile.existsSync()) {
    final s = base64UrlNoPadDecode(keyFile.readAsStringSync().trim(), 32);
    if (s == null) _stop('$keyPath does not hold a 32-byte base64url seed');
    seed = s;
  } else if (recorded != null) {
    _stop(
      'the shipped home app is signed by ${recorded['key']}, and its seed '
      'is not at $keyPath; restore that file (a new key would make the '
      'next release a foreign app to every install)',
    );
  } else if (!write) {
    stdout.writeln(
      'pack_home: would make the home app\'s signing key at $keyPath',
    );
    stdout.writeln(
      'pack_home: would write assets/home/home.clax ($title v$version, release $release, $hash, ${files.length} files)',
    );
    stdout.writeln('pack_home: would write assets/home/release.json');
    return;
  } else {
    final r = Random.secure();
    seed = Uint8List.fromList([for (var i = 0; i < 32; i++) r.nextInt(256)]);
    keyFile.parent.createSync(recursive: true);
    keyFile.writeAsStringSync('${base64UrlNoPad(seed)}\n');
    stdout.writeln(
      'pack_home: made the home app\'s signing key at $keyPath; keep it backed up',
    );
  }
  final publicKey = await publicKeyFromSeed(seed);
  if (recorded != null && recorded['key'] != publicKey) {
    _stop('$keyPath holds key $publicKey, not the shipped ${recorded['key']}');
  }

  final written = await BundleWriter.write(
    lineage: lineage,
    version: version,
    title: title,
    iconWord: iconWord,
    capabilities: capabilities,
    files: files,
    keySeed: seed,
  );
  final releaseJson =
      '${const JsonEncoder.withIndent('  ').convert({'release': release, 'hash': written.manifest.contentSha256, 'version': version, 'lineage': lineage, 'key': publicKey})}\n';

  var changed = false;
  void put(File f, List<int> bytes, String what) {
    final same = f.existsSync() && _equal(f.readAsBytesSync(), bytes);
    if (same) {
      stdout.writeln('pack_home: $what is current');
      return;
    }
    changed = true;
    if (!write) {
      stdout.writeln('pack_home: would write $what');
      return;
    }
    f.parent.createSync(recursive: true);
    final tmp = File('${f.path}.tmp');
    tmp.writeAsBytesSync(bytes, flush: true);
    tmp.renameSync(f.path);
    stdout.writeln('pack_home: wrote $what');
  }

  put(
    bundleFile,
    written.bytes,
    'assets/home/home.clax ($title v$version, release $release, $hash, ${files.length} files, ${written.bytes.length} bytes)',
  );
  put(releaseFile, utf8.encode(releaseJson), 'assets/home/release.json');
  if (changed && !write) {
    stdout.writeln('pack_home: a dry run; nothing was written');
  }
}

bool _equal(List<int> a, List<int> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}
