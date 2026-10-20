/// The hashes an app bundle carries (bundles spec §4.5).
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:cryptography/dart.dart';

const _sha256 = DartSha256();

/// Lower-case hex SHA-256 of [bytes]: a file's hash and the seed's hash.
String sha256Hex(List<int> bytes) => hex(_sha256.hashSync(bytes).bytes);

/// [bytes] as lower-case hex.
String hex(List<int> bytes) {
  final out = StringBuffer();
  for (final v in bytes) {
    out.write(v.toRadixString(16).padLeft(2, '0'));
  }
  return out.toString();
}

/// Orders [a] and [b] by their UTF-8 bytes, the order the format sorts
/// paths in.
int compareUtf8(String a, String b) {
  final x = utf8.encode(a);
  final y = utf8.encode(b);
  final n = x.length < y.length ? x.length : y.length;
  for (var i = 0; i < n; i++) {
    if (x[i] != y[i]) return x[i] - y[i];
  }
  return x.length - y.length;
}

/// The content hash of [files] (path to file hash and size): `sha256:`
/// and the hex SHA-256 of one line `<path>\0<sha256 hex>\0<size>\n` per
/// file in byte order of path. It equals the daemon's version
/// `content_sha256` for the same files, and, over a draft's files but
/// `icon.svg`, the render gate's digest (`clax_core::digest::files_digest`,
/// `POST /api/drafts/{id}/renders`).
String contentHash(Map<String, ({String sha256, int size})> files) {
  final paths = files.keys.toList()..sort(compareUtf8);
  final out = BytesBuilder(copy: false);
  for (final p in paths) {
    final f = files[p]!;
    out.add(utf8.encode('$p\x00${f.sha256}\x00${f.size}\n'));
  }
  return 'sha256:${sha256Hex(out.takeBytes())}';
}
