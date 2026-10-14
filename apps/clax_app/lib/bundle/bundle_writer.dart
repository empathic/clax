/// Writes an app bundle (bundles spec §4): deterministic, so the same
/// manifest, files and seed give the same bytes.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:archive/archive.dart' show Deflate, getCrc32;

import 'bundle_reader.dart';
import 'content_hash.dart';
import 'manifest.dart';
import 'signing.dart';

/// One file of the app: its bytes and its content type.
class BundleFile {
  const BundleFile(this.bytes, {required this.type});

  final Uint8List bytes;
  final String type;
}

/// One zip entry: [name], [bytes], `deflate`d when [deflate] and that is
/// smaller, else `stored`.
class ZipEntry {
  const ZipEntry(this.name, this.bytes, {this.deflate = false});

  final String name;
  final Uint8List bytes;
  final bool deflate;
}

/// A written bundle: the `.clax` bytes and the manifest they carry.
class WrittenBundle {
  const WrittenBundle(this.bytes, this.manifest);

  final Uint8List bytes;
  final Manifest manifest;
}

abstract final class BundleWriter {
  /// The bundle of [files] (path to file) as version [version] of
  /// [lineage], signed by the 32-byte Ed25519 [keySeed]. `icon.svg` is
  /// named when [files] has it; `kit` hashes the files under `kit/`.
  /// [seed], when not empty, is written as `seed.jsonl`.
  ///
  /// The result is read back with [BundleReader.read] before it is
  /// returned, so it throws the [BundleError] a reader would.
  static Future<WrittenBundle> write({
    required String lineage,
    required int version,
    required String title,
    required String? iconWord,
    required Map<String, Object?> capabilities,
    required Map<String, BundleFile> files,
    List<SeedDoc> seed = const [],
    required List<int> keySeed,
    String? authorName,
    ForkedFrom? forkedFrom,
  }) async {
    final entries = {
      for (final MapEntry(:key, :value) in files.entries)
        key: ManifestFile(
          sha256: sha256Hex(value.bytes),
          size: value.bytes.length,
          type: value.type,
        ),
    };
    final kitFiles = {
      for (final MapEntry(:key, :value) in entries.entries)
        if (key.startsWith('kit/'))
          key: (sha256: value.sha256, size: value.size),
    };
    final seedBytes = seed.isEmpty ? null : encodeSeed(seed);
    final manifest = Manifest(
      lineage: lineage,
      version: version,
      title: title,
      icon: ManifestIcon(
        svg: files.containsKey('icon.svg') ? 'icon.svg' : null,
        word: iconWord,
      ),
      capabilities: capabilities,
      kit: kitFiles.isEmpty ? null : contentHash(kitFiles),
      files: entries,
      contentSha256: contentHash({
        for (final MapEntry(:key, :value) in entries.entries)
          key: (sha256: value.sha256, size: value.size),
      }),
      seed: seedBytes == null
          ? null
          : ManifestSeed(
              sha256: sha256Hex(seedBytes),
              size: seedBytes.length,
              docs: seed.length,
            ),
      author: ManifestAuthor(
        key: await publicKeyFromSeed(keySeed),
        name: authorName,
      ),
      forkedFrom: forkedFrom,
    );
    final manifestBytes = manifest.encode();
    final signature = await signManifest(manifestBytes, keySeed);
    final bytes = zip([
      ZipEntry('manifest.json', manifestBytes, deflate: true),
      ZipEntry('manifest.sig', Uint8List.fromList(ascii.encode(signature))),
      for (final p in files.keys.toList()..sort(compareUtf8))
        ZipEntry('files/$p', files[p]!.bytes, deflate: true),
      if (seedBytes != null) ZipEntry('seed.jsonl', seedBytes, deflate: true),
    ]);
    final read = await BundleReader.read(bytes);
    return WrittenBundle(bytes, read.manifest);
  }

  /// `seed.jsonl`: one `{"path", "data"}` line per document, sorted by
  /// path in byte order, `\n` after each.
  static Uint8List encodeSeed(List<SeedDoc> docs) {
    final sorted = [...docs]..sort((a, b) => compareUtf8(a.path, b.path));
    final out = StringBuffer();
    for (final d in sorted) {
      out.write(jsonEncode({'path': d.path, 'data': d.data}));
      out.write('\n');
    }
    return Uint8List.fromList(utf8.encode(out.toString()));
  }

  /// A zip of [entries] in the given order (APPNOTE 6.3, no zip64, no
  /// encryption, no directory entries, extra fields or comments; every
  /// timestamp 1980-01-01 00:00).
  static Uint8List zip(List<ZipEntry> entries) {
    final out = BytesBuilder(copy: false);
    final central = BytesBuilder(copy: false);
    for (final e in entries) {
      final name = ascii.encode(e.name);
      var method = 0;
      var data = e.bytes;
      if (e.deflate && e.bytes.isNotEmpty) {
        final deflated = Deflate(e.bytes).getBytes();
        if (deflated.length < e.bytes.length) {
          method = 8;
          data = deflated;
        }
      }
      final crc = getCrc32(e.bytes);
      final offset = out.length;
      final local = ByteData(30)
        ..setUint32(0, 0x04034b50, Endian.little)
        ..setUint16(4, 20, Endian.little)
        ..setUint16(6, 0, Endian.little)
        ..setUint16(8, method, Endian.little)
        ..setUint16(10, 0, Endian.little) // 00:00
        ..setUint16(12, _dosDate1980, Endian.little)
        ..setUint32(14, crc, Endian.little)
        ..setUint32(18, data.length, Endian.little)
        ..setUint32(22, e.bytes.length, Endian.little)
        ..setUint16(26, name.length, Endian.little)
        ..setUint16(28, 0, Endian.little);
      out
        ..add(local.buffer.asUint8List())
        ..add(name)
        ..add(data);
      final c = ByteData(46)
        ..setUint32(0, 0x02014b50, Endian.little)
        ..setUint16(4, 20, Endian.little)
        ..setUint16(6, 20, Endian.little)
        ..setUint16(8, 0, Endian.little)
        ..setUint16(10, method, Endian.little)
        ..setUint16(12, 0, Endian.little)
        ..setUint16(14, _dosDate1980, Endian.little)
        ..setUint32(16, crc, Endian.little)
        ..setUint32(20, data.length, Endian.little)
        ..setUint32(24, e.bytes.length, Endian.little)
        ..setUint16(28, name.length, Endian.little)
        ..setUint32(42, offset, Endian.little);
      central
        ..add(c.buffer.asUint8List())
        ..add(name);
    }
    final cdOffset = out.length;
    final cdSize = central.length;
    out.add(central.takeBytes());
    final end = ByteData(22)
      ..setUint32(0, 0x06054b50, Endian.little)
      ..setUint16(8, entries.length, Endian.little)
      ..setUint16(10, entries.length, Endian.little)
      ..setUint32(12, cdSize, Endian.little)
      ..setUint32(16, cdOffset, Endian.little);
    out.add(end.buffer.asUint8List());
    return out.takeBytes();
  }
}

/// 1980-01-01 in MS-DOS date form: year 0, month 1, day 1.
const _dosDate1980 = (0 << 9) | (1 << 5) | 1;
