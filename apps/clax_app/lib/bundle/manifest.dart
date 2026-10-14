/// An app bundle's `manifest.json` (bundles spec §4.2): its schema, the
/// rules the schema cannot say, and the writer's byte form.
library;

import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'content_hash.dart';

/// Why a bundle was refused. [code] is one of `too_large`, `not_a_bundle`,
/// `too_new`, `bad_manifest`, `bad_signature`, `bad_file`, `bad_seed`,
/// `not_shareable`; [message] says which rule, for logs and Details.
class BundleError implements Exception {
  const BundleError(this.code, this.message);

  final String code;
  final String message;

  @override
  String toString() => 'BundleError($code): $message';
}

/// The bundle format this Clax reads and writes.
const bundleFormat = 1;

/// At most this many files in a bundle.
const maxBundleFiles = 512;

/// At most this many bytes in one file.
const maxFileBytes = 16 * 1024 * 1024;

/// At most this many bytes in all of a bundle's files.
const maxFilesBytes = 64 * 1024 * 1024;

/// At most this many bytes of `seed.jsonl`.
const maxSeedBytes = 8 * 1024 * 1024;

/// At most this many seed documents.
const maxSeedDocs = 2000;

final _pathRe = RegExp(
  r'^(?!.*(//|/$|(^|/)\.{1,2}(/|$)))[A-Za-z0-9_][A-Za-z0-9_.\-/]{0,199}$',
);
final _lineageRe = RegExp(r'^l_[a-z2-7]{26}$');
final _wordRe = RegExp(r'^[a-z][a-z0-9-]{0,31}$');
final _prefixedHashRe = RegExp(r'^sha256:[0-9a-f]{64}$');
final _hashRe = RegExp(r'^[0-9a-f]{64}$');
final _keyRe = RegExp(r'^[A-Za-z0-9_-]{43}$');

/// Whether [path] is a valid bundle file path (§6.3): relative,
/// `/`-separated, ASCII letters, digits and `_ . - /`, no empty, `.` or
/// `..` segment, not starting with `.`, at most 200 bytes.
bool isBundlePath(String path) => _pathRe.hasMatch(path);

/// One file's entry in the manifest.
class ManifestFile {
  const ManifestFile({
    required this.sha256,
    required this.size,
    required this.type,
  });

  /// Lower-case hex SHA-256 of the bytes.
  final String sha256;
  final int size;

  /// The content type the file is published with.
  final String type;
}

/// The manifest's `icon`.
class ManifestIcon {
  const ManifestIcon({required this.svg, required this.word});

  /// `icon.svg` when the bundle carries that file, else null.
  final String? svg;

  /// The icon word, or null.
  final String? word;
}

/// The manifest's `seed`: the hash, size and line count of `seed.jsonl`.
class ManifestSeed {
  const ManifestSeed({
    required this.sha256,
    required this.size,
    required this.docs,
  });

  final String sha256;
  final int size;
  final int docs;
}

/// The manifest's `author`.
class ManifestAuthor {
  const ManifestAuthor({required this.key, required this.name});

  /// The raw Ed25519 public key, base64url without padding.
  final String key;

  /// The sender's own name for themselves, or null.
  final String? name;
}

/// The manifest's `forked_from`: the lineage, version and key a fork was
/// made from.
class ForkedFrom {
  const ForkedFrom({
    required this.lineage,
    required this.version,
    required this.key,
  });

  final String lineage;
  final int version;
  final String key;
}

/// One seed document: its path and its data.
class SeedDoc {
  const SeedDoc(this.path, this.data);

  final String path;
  final Map<String, Object?> data;
}

/// A parsed, valid `manifest.json`.
class Manifest {
  /// A manifest from its fields, checked as [parse] checks a read one.
  Manifest({
    required this.lineage,
    required this.version,
    required this.title,
    required this.icon,
    required this.capabilities,
    required this.kit,
    required Map<String, ManifestFile> files,
    required this.contentSha256,
    required this.seed,
    required this.author,
    required this.forkedFrom,
  }) : files = Map.unmodifiable(files) {
    _checkJson(toJson());
  }

  Manifest._(
    this.lineage,
    this.version,
    this.title,
    this.icon,
    this.capabilities,
    this.kit,
    this.files,
    this.contentSha256,
    this.seed,
    this.author,
    this.forkedFrom,
  );

  /// Parses and checks [bytes] (§4.2 and its further rules). Throws
  /// [BundleError]: `too_new` for a format above [bundleFormat],
  /// `not_shareable` for `db.read` or `db.share`, else `bad_manifest`.
  static Manifest parse(Uint8List bytes) {
    if (bytes.length >= 3 &&
        bytes[0] == 0xef &&
        bytes[1] == 0xbb &&
        bytes[2] == 0xbf) {
      throw _bad('manifest.json starts with a byte order mark');
    }
    final Object? json;
    try {
      json = jsonDecode(utf8.decode(bytes));
    } on FormatException catch (e) {
      throw _bad('manifest.json is not UTF-8 JSON: ${e.message}');
    }
    return _checkJson(json);
  }

  final int format = bundleFormat;
  final String lineage;
  final int version;
  final String title;
  final ManifestIcon icon;

  /// The version's full capability declaration, passed to the daemon
  /// unchanged.
  final Map<String, Object?> capabilities;

  /// The content hash of the files under `kit/`, or null.
  final String? kit;

  /// Path to entry.
  final Map<String, ManifestFile> files;

  /// `sha256:` and the hex content hash (§4.5).
  final String contentSha256;
  final ManifestSeed? seed;
  final ManifestAuthor author;
  final ForkedFrom? forkedFrom;

  /// The manifest as JSON, keys in the schema's `required` order and
  /// `files` in byte order of path.
  Map<String, Object?> toJson() => {
    'format': format,
    'lineage': lineage,
    'version': version,
    'title': title,
    'icon': {'svg': icon.svg, 'word': icon.word},
    'capabilities': capabilities,
    'kit': kit,
    'files': {
      for (final p in files.keys.toList()..sort(compareUtf8))
        p: {
          'sha256': files[p]!.sha256,
          'size': files[p]!.size,
          'type': files[p]!.type,
        },
    },
    'content_sha256': contentSha256,
    'seed': switch (seed) {
      null => null,
      final s => {'sha256': s.sha256, 'size': s.size, 'docs': s.docs},
    },
    'author': {'key': author.key, 'name': author.name},
    'forked_from': switch (forkedFrom) {
      null => null,
      final f => {'lineage': f.lineage, 'version': f.version, 'key': f.key},
    },
  };

  /// The writer's bytes: UTF-8, [toJson]'s key order, two spaces of
  /// indentation, `\n` line ends and a final `\n`.
  Uint8List encode() => Uint8List.fromList(
    utf8.encode('${const JsonEncoder.withIndent('  ').convert(toJson())}\n'),
  );
}

BundleError _bad(String message) => BundleError('bad_manifest', message);

Map<String, Object?> _object(
  Object? v,
  String what,
  List<String> required, {
  bool closed = true,
}) {
  if (v is! Map<String, Object?>) throw _bad('$what is not an object');
  for (final k in required) {
    if (!v.containsKey(k)) throw _bad('$what has no "$k"');
  }
  if (closed) {
    for (final k in v.keys) {
      if (!required.contains(k)) throw _bad('$what has an unknown key "$k"');
    }
  }
  return v;
}

String _string(Object? v, String what, {RegExp? re, int? min, int? max}) {
  if (v is! String) throw _bad('$what is not a string');
  final n = v.runes.length;
  if ((min != null && n < min) ||
      (max != null && n > max) ||
      (re != null && !re.hasMatch(v))) {
    throw _bad('$what is not valid');
  }
  return v;
}

String? _stringOrNull(
  Object? v,
  String what, {
  RegExp? re,
  int? min,
  int? max,
}) => v == null ? null : _string(v, what, re: re, min: min, max: max);

int _int(Object? v, String what, int min, [int? max]) {
  if (v is! int || v < min || (max != null && v > max)) {
    throw _bad('$what is not an integer in range');
  }
  return v;
}

Manifest _checkJson(Object? json) {
  if (json is! Map<String, Object?>) {
    throw _bad('manifest.json is not an object');
  }
  final format = json['format'];
  if (format is int && format > bundleFormat) {
    throw const BundleError(
      'too_new',
      'Made by a newer Clax: update Clax to open it',
    );
  }
  final m = _object(json, 'the manifest', const [
    'format', 'lineage', 'version', 'title', 'icon', 'capabilities', 'kit',
    'files', 'content_sha256', 'seed', 'author', 'forked_from', //
  ]);
  if (format is! int || format != bundleFormat) {
    throw _bad('format is not 1');
  }

  final iconJson = _object(m['icon'], 'icon', const ['svg', 'word']);
  final svg = iconJson['svg'];
  if (svg != null && svg != 'icon.svg') throw _bad('icon.svg is not valid');
  final icon = ManifestIcon(
    svg: svg as String?,
    word: _stringOrNull(iconJson['word'], 'icon.word', re: _wordRe),
  );

  final caps = m['capabilities'];
  if (caps is! Map<String, Object?>) {
    throw _bad('capabilities is not an object');
  }
  if (caps['db'] case final Map<String, Object?> db
      when db.containsKey('read') || db.containsKey('share')) {
    throw const BundleError(
      'not_shareable',
      'The bundle declares db.read or db.share, which are not shared',
    );
  }

  final filesJson = m['files'];
  if (filesJson is! Map<String, Object?>) throw _bad('files is not an object');
  if (filesJson.isEmpty || filesJson.length > maxBundleFiles) {
    throw _bad('files holds 0 or more than $maxBundleFiles entries');
  }
  final files = <String, ManifestFile>{};
  final folded = <String>{};
  var total = 0;
  for (final MapEntry(:key, :value) in filesJson.entries) {
    if (!isBundlePath(key)) {
      throw _bad('${jsonEncode(key)} is not a valid path');
    }
    if (!folded.add(key.toLowerCase())) {
      throw _bad('${jsonEncode(key)} repeats a path ignoring case');
    }
    final f = _object(value, 'files[$key]', const ['sha256', 'size', 'type']);
    final size = _int(f['size'], 'files[$key].size', 0, maxFileBytes);
    total += size;
    files[key] = ManifestFile(
      sha256: _string(f['sha256'], 'files[$key].sha256', re: _hashRe),
      size: size,
      type: _string(f['type'], 'files[$key].type', max: 100),
    );
  }
  if (!files.containsKey('index.html')) throw _bad('files has no index.html');
  if (icon.svg != null && !files.containsKey('icon.svg')) {
    throw _bad('icon names icon.svg but files has none');
  }
  if (total > maxFilesBytes) throw _bad('the files are over 64 MiB in all');

  ManifestSeed? seed;
  if (m['seed'] != null) {
    final s = _object(m['seed'], 'seed', const ['sha256', 'size', 'docs']);
    seed = ManifestSeed(
      sha256: _string(s['sha256'], 'seed.sha256', re: _hashRe),
      size: _int(s['size'], 'seed.size', 1, maxSeedBytes),
      docs: _int(s['docs'], 'seed.docs', 1, maxSeedDocs),
    );
  }

  final a = _object(m['author'], 'author', const ['key', 'name']);
  final author = ManifestAuthor(
    key: _string(a['key'], 'author.key', re: _keyRe),
    name: _stringOrNull(a['name'], 'author.name', min: 1, max: 64),
  );

  ForkedFrom? forkedFrom;
  if (m['forked_from'] != null) {
    final f = _object(m['forked_from'], 'forked_from', const [
      'lineage', 'version', 'key', //
    ]);
    forkedFrom = ForkedFrom(
      lineage: _string(f['lineage'], 'forked_from.lineage', re: _lineageRe),
      version: _int(f['version'], 'forked_from.version', 1),
      key: _string(f['key'], 'forked_from.key', re: _keyRe),
    );
  }

  return Manifest._(
    _string(m['lineage'], 'lineage', re: _lineageRe),
    _int(m['version'], 'version', 1, 2147483647),
    _string(m['title'], 'title', min: 1, max: 200),
    icon,
    caps,
    _stringOrNull(m['kit'], 'kit', re: _prefixedHashRe),
    Map.unmodifiable(files),
    _string(m['content_sha256'], 'content_sha256', re: _prefixedHashRe),
    seed,
    author,
    forkedFrom,
  );
}

/// A new lineage ID: `l_` and 16 random bytes in lower-case base32.
String newLineage([Random? random]) {
  final r = random ?? Random.secure();
  return 'l_${base32([for (var i = 0; i < 16; i++) r.nextInt(256)])}';
}

/// [bytes] in RFC 4648 base32 without padding, lower-case unless [upper].
String base32(List<int> bytes, {bool upper = false}) {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz234567';
  final out = StringBuffer();
  var buffer = 0;
  var bits = 0;
  for (final v in bytes) {
    buffer = (buffer << 8) | v;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out.write(alphabet[(buffer >> bits) & 31]);
    }
    buffer &= (1 << bits) - 1;
  }
  if (bits > 0) out.write(alphabet[(buffer << (5 - bits)) & 31]);
  final s = out.toString();
  return upper ? s.toUpperCase() : s;
}
