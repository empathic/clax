/// Reads and verifies an app bundle (bundles spec §6.1–6.3), in memory:
/// nothing is written to disk.
library;

import 'dart:convert';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:archive/archive.dart'
    show InputMemoryStream, Inflate, OutputMemoryStream, InputStream;

import '../agent/icon_svg.dart';
import 'content_hash.dart';
import 'manifest.dart';
import 'signing.dart';

export 'manifest.dart' show BundleError, Manifest, SeedDoc;

/// At most this many bytes in a `.clax` file.
const maxBundleBytes = 32 * 1024 * 1024;

/// At most this many zip entries: [maxBundleFiles] files and three.
const maxBundleEntries = maxBundleFiles + 3;

/// At most this many bytes of `manifest.json`, inflated.
const maxManifestBytes = 256 * 1024;

/// A read and verified bundle.
class Bundle {
  const Bundle({
    required this.bytes,
    required this.manifest,
    required this.manifestBytes,
    required this.signature,
    required this.files,
    required this.seed,
    required this.iconSvg,
  });

  /// The `.clax` file as read, kept byte for byte for re-share.
  final Uint8List bytes;
  final Manifest manifest;

  /// The `manifest.json` entry's bytes, which the signature covers.
  final Uint8List manifestBytes;

  /// `manifest.sig`.
  final String signature;

  /// Path to bytes, one per manifest file.
  final Map<String, Uint8List> files;

  /// The seed documents in file order, or null when the bundle has none.
  final List<SeedDoc>? seed;

  /// `icon.svg` cleaned by [cleanIcon], or null when the bundle has none
  /// or it cannot be drawn.
  final String? iconSvg;
}

/// Files larger than this are read on a background isolate.
const inlineReadBytes = 16 * 1024;

abstract final class BundleReader {
  /// Reads and verifies [bytes] in §6.2's order, or throws [BundleError].
  ///
  /// A file over [inlineReadBytes] is read on a background isolate
  /// (`Isolate.run`), so a costly file never blocks the caller's isolate;
  /// a smaller one is read in place, its work bounded by its size.
  static Future<Bundle> read(Uint8List bytes) {
    if (bytes.length > inlineReadBytes) {
      return Isolate.run(() => _read(bytes));
    }
    return _read(bytes);
  }

  static Future<Bundle> _read(Uint8List bytes) async {
    // 1. The file and its zip structure.
    if (bytes.length > maxBundleBytes) {
      throw const BundleError('too_large', 'The file is over 32 MiB');
    }
    final entries = _entries(bytes);

    // 2. The manifest.
    final mEntry = entries['manifest.json'];
    if (mEntry == null) throw _notBundle('it has no manifest.json');
    final manifestBytes = mEntry.inflate(bytes, maxManifestBytes);
    if (manifestBytes == null) {
      throw const BundleError('too_large', 'manifest.json is over 256 KiB');
    }
    final manifest = Manifest.parse(manifestBytes);

    // 3. The signature.
    final sEntry = entries['manifest.sig'];
    if (sEntry == null) throw _notBundle('it has no manifest.sig');
    final sigBytes = sEntry.inflate(bytes, 86);
    final signature = sigBytes == null ? null : _ascii(sigBytes);
    if (signature == null ||
        !await verifyManifest(manifestBytes, signature, manifest.author.key)) {
      throw const BundleError(
        'bad_signature',
        'manifest.sig is not the author key\'s signature of manifest.json',
      );
    }

    // 4. The files.
    final files = <String, Uint8List>{};
    for (final name in entries.keys) {
      if (name.startsWith('files/') &&
          !manifest.files.containsKey(name.substring(6))) {
        throw _badFile('$name is not in the manifest');
      }
    }
    for (final MapEntry(key: path, value: f) in manifest.files.entries) {
      final e = entries['files/$path'];
      if (e == null) throw _badFile('files/$path is missing');
      final data = e.inflate(bytes, f.size);
      if (data == null || data.length != f.size) {
        throw _badFile('files/$path is not ${f.size} bytes');
      }
      if (sha256Hex(data) != f.sha256) {
        throw _badFile('files/$path does not match its hash');
      }
      files[path] = data;
    }
    final content = contentHash({
      for (final MapEntry(:key, :value) in manifest.files.entries)
        key: (sha256: value.sha256, size: value.size),
    });
    if (content != manifest.contentSha256) {
      throw _badFile('content_sha256 does not match the files');
    }

    // 5. The seed.
    final seedEntry = entries['seed.jsonl'];
    List<SeedDoc>? seed;
    if (manifest.seed case final s?) {
      if (seedEntry == null) throw _badSeed('seed.jsonl is missing');
      final data = seedEntry.inflate(bytes, s.size);
      if (data == null || data.length != s.size) {
        throw _badSeed('seed.jsonl is not ${s.size} bytes');
      }
      if (sha256Hex(data) != s.sha256) {
        throw _badSeed('seed.jsonl does not match its hash');
      }
      seed = _seedDocs(data, s.docs);
    } else if (seedEntry != null) {
      throw _badSeed('seed.jsonl is present but the manifest has no seed');
    }

    // 6. The icon.
    String? iconSvg;
    if (manifest.icon.svg != null) {
      iconSvg = cleanIcon(
        utf8.decode(files['icon.svg']!, allowMalformed: true),
      );
    }

    return Bundle(
      bytes: bytes,
      manifest: manifest,
      manifestBytes: manifestBytes,
      signature: signature,
      files: Map.unmodifiable(files),
      seed: seed,
      iconSvg: iconSvg,
    );
  }
}

/// The most deflate blocks an entry of [max] bytes may have: two, and one
/// per 8 KiB. Encoders (zlib, `archive`'s `Deflate`) close a block only
/// after 16,384 symbols, at least 16 KiB of output, so only the last block
/// of a real stream is short; a stream of many small or empty blocks is
/// refused before it costs more than its size allows.
int maxDeflateBlocks(int max) => 2 + max ~/ 8192;

/// [data], a raw deflate stream, inflated: null when it holds more than
/// [max] bytes (inflating stops before it would pass [max], whatever any
/// header says), more than [maxDeflateBlocks] blocks, or is not valid
/// deflate. The stream is read with two zero bytes after it: `archive`'s
/// decoder looks a whole longest code ahead before each code, so a stream
/// whose last code is shorter than that would otherwise end early. A
/// stream cut short may then read as a shorter one; the reader's size and
/// hash checks refuse it.
Uint8List? inflateAtMost(Uint8List data, int max) {
  final out = _CappedOutput(max);
  final padded = Uint8List(data.length + _lookahead)..setRange(0, data.length, data);
  try {
    final inflate = Inflate.lazy(InputMemoryStream(padded), output: out);
    var blocks = 1;
    final most = maxDeflateBlocks(max);
    while (inflate.inflateBlock()) {
      if (++blocks > most) return null;
    }
  } on _Overflow {
    return null;
  } on Object {
    return null;
  }
  return out.getBytes();
}

/// The bytes after a stream that cover `archive`'s lookahead of up to 15
/// bits past its last code.
const _lookahead = 2;

class _Overflow implements Exception {}

/// An output that throws [_Overflow] before it would hold more than
/// [max] bytes plus one.
class _CappedOutput extends OutputMemoryStream {
  _CappedOutput(this.max) : super(size: 1024);

  final int max;

  void _room(int n) {
    if (length + n > max) throw _Overflow();
  }

  @override
  void writeByte(int value) {
    _room(1);
    super.writeByte(value);
  }

  @override
  void writeBytes(List<int> bytes, {int? length}) {
    _room(length ?? bytes.length);
    super.writeBytes(bytes, length: length);
  }

  @override
  void writeStream(InputStream stream) {
    _room(stream.length);
    super.writeStream(stream);
  }

  @override
  void writeBackReference(int distance, int count) {
    _room(count);
    super.writeBackReference(distance, count);
  }
}

BundleError _notBundle(String why) =>
    BundleError('not_a_bundle', 'This is not a Clax app bundle: $why');

BundleError _badFile(String why) => BundleError('bad_file', why);

BundleError _badSeed(String why) => BundleError('bad_seed', why);

String? _ascii(Uint8List b) {
  for (final v in b) {
    if (v > 0x7e || v < 0x20) return null;
  }
  return ascii.decode(b);
}

/// One central-directory entry.
class _Entry {
  _Entry(this.method, this.dataStart, this.compressedSize);

  final int method;
  final int dataStart;
  final int compressedSize;

  /// The entry's bytes, or null past [max] bytes, when its compressed
  /// size is over [max] plus 1 KiB (deflate never needs more for data it
  /// shrinks), or when damaged.
  Uint8List? inflate(Uint8List zip, int max) {
    if (compressedSize > max + 1024) return null;
    final raw = Uint8List.sublistView(
      zip,
      dataStart,
      dataStart + compressedSize,
    );
    if (method == 0) return raw.length > max ? null : raw;
    return inflateAtMost(raw, max);
  }
}

bool _layoutName(String n) =>
    n == 'manifest.json' ||
    n == 'manifest.sig' ||
    n == 'seed.jsonl' ||
    (n.startsWith('files/') && isBundlePath(n.substring(6)));

/// The zip's entries by name, checked as §6.2 step 1 says.
Map<String, _Entry> _entries(Uint8List z) {
  final d = ByteData.sublistView(z);
  int u16(int at) => d.getUint16(at, Endian.little);
  int u32(int at) => d.getUint32(at, Endian.little);

  // The end of central directory record closes the file: no comment.
  if (z.length < 22) throw _notBundle('it is too short to be a zip');
  final eocd = z.length - 22;
  if (u32(eocd) != 0x06054b50) {
    throw _notBundle('it has no zip end record');
  }
  if (z.length >= 42 && u32(z.length - 42) == 0x07064b50) {
    throw _notBundle('it is a zip64 archive');
  }
  final count = u16(eocd + 10);
  final cdSize = u32(eocd + 12);
  final cdOffset = u32(eocd + 16);
  if (u16(eocd + 4) != 0 ||
      u16(eocd + 6) != 0 ||
      u16(eocd + 8) != count ||
      count == 0xffff ||
      cdSize == 0xffffffff ||
      cdOffset == 0xffffffff) {
    throw _notBundle('it is a zip64 or multi-part archive');
  }
  if (u16(eocd + 20) != 0) throw _notBundle('the zip has a comment');
  if (count > maxBundleEntries) {
    throw const BundleError('too_large', 'The bundle has too many entries');
  }
  if (cdOffset + cdSize != eocd) {
    throw _notBundle('its central directory is not where the end record says');
  }

  final out = <String, _Entry>{};
  var p = cdOffset;
  // Entries sit end to end from offset 0, in central-directory order, and
  // the last one ends where the central directory starts: no bytes
  // before, between or after them.
  var next = 0;
  for (var i = 0; i < count; i++) {
    if (p + 46 > eocd || u32(p) != 0x02014b50) {
      throw _notBundle('its central directory is damaged');
    }
    final flags = u16(p + 8);
    final method = u16(p + 10);
    final csize = u32(p + 20);
    final usize = u32(p + 24);
    final nameLen = u16(p + 28);
    final extraLen = u16(p + 30);
    final commentLen = u16(p + 32);
    final local = u32(p + 42);
    if (p + 46 + nameLen > eocd) {
      throw _notBundle('its central directory is damaged');
    }
    final nameBytes = Uint8List.sublistView(z, p + 46, p + 46 + nameLen);
    final name = _ascii(nameBytes);
    if (flags & 1 != 0) throw _notBundle('an entry is encrypted');
    if (csize == 0xffffffff || usize == 0xffffffff || local == 0xffffffff) {
      throw _notBundle('an entry is zip64');
    }
    if (flags & ~0x0800 != 0) {
      throw _notBundle('an entry uses unsupported zip flags');
    }
    if (method != 0 && method != 8) {
      throw _notBundle('an entry uses compression method $method');
    }
    if (extraLen != 0 || commentLen != 0) {
      throw _notBundle('an entry has extra fields or a comment');
    }
    if (name == null || !_layoutName(name)) {
      throw _notBundle('it has an entry ${jsonEncode(name ?? '?')}');
    }
    if (out.containsKey(name)) throw _notBundle('it repeats the entry $name');

    // The local header must agree on the name, method and flags.
    if (local + 30 > cdOffset || u32(local) != 0x04034b50) {
      throw _notBundle('the local header of $name is damaged');
    }
    final lNameLen = u16(local + 26);
    final lExtraLen = u16(local + 28);
    final dataStart = local + 30 + lNameLen + lExtraLen;
    if (local != next || lExtraLen != 0) {
      throw _notBundle('it has bytes outside its entries before $name');
    }
    if (lNameLen != nameLen ||
        u16(local + 6) != flags ||
        u16(local + 8) != method ||
        dataStart + csize > cdOffset) {
      throw _notBundle('the local header of $name disagrees');
    }
    for (var j = 0; j < nameLen; j++) {
      if (z[local + 30 + j] != nameBytes[j]) {
        throw _notBundle('the local header of $name disagrees');
      }
    }
    out[name] = _Entry(method, dataStart, csize);
    next = dataStart + csize;
    p += 46 + nameLen;
  }
  if (next != cdOffset || p != eocd) {
    throw _notBundle('it has bytes outside its entries');
  }
  return out;
}

final _seedLineKeys = {'path', 'data'};

/// The documents of `seed.jsonl` [data], checked as §6.2 step 5 says.
List<SeedDoc> _seedDocs(Uint8List data, int docs) {
  final String text;
  try {
    text = utf8.decode(data);
  } on FormatException {
    throw _badSeed('seed.jsonl is not UTF-8');
  }
  if (!text.endsWith('\n')) throw _badSeed('seed.jsonl does not end in \\n');
  final lines = text.substring(0, text.length - 1).split('\n');
  if (lines.length > maxSeedDocs || lines.length != docs) {
    throw _badSeed('seed.jsonl has ${lines.length} lines, not $docs');
  }
  final seen = <String>{};
  final out = <SeedDoc>[];
  for (final (i, line) in lines.indexed) {
    final Object? json;
    try {
      json = jsonDecode(line);
    } on FormatException {
      throw _badSeed('seed.jsonl line ${i + 1} is not JSON');
    }
    if (json is! Map<String, Object?> ||
        json.length != 2 ||
        !json.keys.every(_seedLineKeys.contains)) {
      throw _badSeed('seed.jsonl line ${i + 1} is not {path, data}');
    }
    final path = json['path'];
    final value = json['data'];
    if (path is! String || value is! Map<String, Object?>) {
      throw _badSeed('seed.jsonl line ${i + 1} is not {path, data}');
    }
    if (!withinJsonDepth(value)) {
      throw _badSeed(
        'seed.jsonl line ${i + 1} nests deeper than $maxJsonDepth levels',
      );
    }
    if (!isSeedPath(path)) {
      throw _badSeed(
        'seed.jsonl line ${i + 1} has the path ${jsonEncode(path)}',
      );
    }
    if (!seen.add(path)) throw _badSeed('seed.jsonl repeats $path');
    out.add(SeedDoc(path, value));
  }
  return out;
}

/// Whether [path] may be a seed document's: a daemon document path (at
/// most 1,000 bytes and 16 segments, an even number of them, each 1 to
/// 200 bytes of ASCII letters, digits and `_ - . ~ : @ +`, neither `.` nor
/// `..`) not under `data/users/`.
bool isSeedPath(String path) {
  if (path.length > 1000 || path.startsWith('data/users/')) return false;
  final segs = path.split('/');
  if (segs.length > 16 || segs.length.isOdd) return false;
  for (final s in segs) {
    if (s.isEmpty || s == '.' || s == '..' || s.length > 200) return false;
    for (final c in s.codeUnits) {
      final ok =
          (c >= 0x30 && c <= 0x39) ||
          (c >= 0x41 && c <= 0x5a) ||
          (c >= 0x61 && c <= 0x7a) ||
          '_-.~:@+'.codeUnits.contains(c);
      if (!ok) return false;
    }
  }
  return true;
}
