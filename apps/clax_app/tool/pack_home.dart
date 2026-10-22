// Packs the home app's source (`home/`) into the bundle the Clax app ships
// (`home.clax`) and its release record (`release.json`), written into the
// directory `--out` names. The build runs it: //apps/clax_app:home_bundle
// under Bazel, scripts/build-home-bundle.sh natively, with the version and
// seed apps/clax_app/tool/home-version.sh gives.
//
//   pack_home --app <dir> --key <seed file> --version <n> --out <dir> [--dev]
//
// `--app` names apps/clax_app. `home/manifest.json` gives the bundle's
// title, icon word, capabilities, lineage, release key and release series,
// and the kit files (from `assets/kit/`) the bundle carries under `kit/`;
// every other file under `home/` is carried as it is. The shipped release
// is `<release>.<version>`, so each version has a release of its own (an
// install's "Keep mine" holds one release, home_update.dart). The bundle
// is deterministic (BundleWriter): the same sources, version and key give
// the same bytes.
//
// The seed file holds the 32 bytes of an Ed25519 seed in base64url without
// padding. Its public half must be the manifest's `key`: every release is
// signed by that one key, or the trust rules (lib/bundle/trust.dart) treat
// it as a foreign lineage. With `--dev` the seed is the development key's
// (tool/home-dev.key) and the bundle takes the manifest's `dev_lineage`, a
// different app to an install of the released one.
//
// Exits 0 when the outputs were written, 1 when packing stops, 2 on a
// usage error.
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_writer.dart';
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
/// Links are followed: Bazel's sandbox gives the sources as links.
Map<String, File> _tree(Directory dir) => {
  for (final e in dir.listSync(recursive: true, followLinks: true))
    if (e is File)
      e.path.substring(dir.path.length + 1).replaceAll(r'\', '/'): e,
};

Future<void> main(List<String> args) async {
  String? keyPath, appPath, outPath, versionArg;
  var dev = false;
  for (var i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--dev':
        dev = true;
      case '--key' when i + 1 < args.length:
        keyPath = args[++i];
      case '--app' when i + 1 < args.length:
        appPath = args[++i];
      case '--out' when i + 1 < args.length:
        outPath = args[++i];
      case '--version' when i + 1 < args.length:
        versionArg = args[++i];
      default:
        _stop(
          'usage: pack_home --app <dir> --key <seed file> --version <n> '
          '--out <dir> [--dev]',
          2,
        );
    }
  }
  if (keyPath == null || appPath == null || outPath == null) {
    _stop('--app, --key, --version and --out are required', 2);
  }
  final version = int.tryParse(versionArg?.trim() ?? '');
  if (version == null || version < 1) _stop('--version must be a positive integer', 2);

  final source = Directory('$appPath/home');
  final kit = Directory('$appPath/assets/kit');

  final m = jsonDecode(
    File('${source.path}/manifest.json').readAsStringSync(),
  ) as Map<String, Object?>;
  final title = m['title'] as String;
  final iconWord = m['icon'] as String?;
  final capabilities = (m['capabilities'] as Map).cast<String, Object?>();
  final lineage = (dev ? m['dev_lineage'] : m['lineage']) as String;
  final releaseKey = m['key'] as String;
  final series = m['release'] as String;
  final floor = m['version'] as int;
  final kitFiles = (m['kit'] as List).cast<String>();
  if (version <= floor) {
    _stop('version $version is not above home/manifest.json\'s $floor');
  }
  final release = '$series.$version';

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

  final keyFile = File(keyPath);
  if (!keyFile.existsSync()) _stop('no seed at $keyPath');
  final Uint8List? seed = base64UrlNoPadDecode(
    keyFile.readAsStringSync().trim(),
    32,
  );
  if (seed == null) _stop('$keyPath does not hold a 32-byte base64url seed');
  final publicKey = await publicKeyFromSeed(seed);
  if (!dev && publicKey != releaseKey) {
    _stop(
      '$keyPath holds key $publicKey, not the home app\'s release key '
      '$releaseKey (home/manifest.json); a release under another key is a '
      'foreign app to every install',
    );
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

  void put(String name, List<int> bytes) {
    final f = File('$outPath/$name');
    f.parent.createSync(recursive: true);
    final tmp = File('${f.path}.tmp');
    tmp.writeAsBytesSync(bytes, flush: true);
    tmp.renameSync(f.path);
  }

  put('home.clax', written.bytes);
  put('release.json', utf8.encode(releaseJson));
  stdout.writeln(
    'pack_home: $title v$version, release $release${dev ? ' (development key)' : ''}, '
    '${written.manifest.contentSha256}, ${files.length} files, '
    '${written.bytes.length} bytes',
  );
}
