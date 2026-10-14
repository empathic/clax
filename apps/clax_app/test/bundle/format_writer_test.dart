import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/manifest.dart';
import 'package:clax_app/bundle/signing.dart';
import 'package:flutter_test/flutter_test.dart';

import 'format_fixtures.dart';

/// Entry names of a zip, in central-directory order.
List<String> _names(Uint8List zip) {
  final d = ByteData.sublistView(zip);
  final eocd = zip.length - 22;
  final count = d.getUint16(eocd + 10, Endian.little);
  var p = d.getUint32(eocd + 16, Endian.little);
  final out = <String>[];
  for (var i = 0; i < count; i++) {
    final n = d.getUint16(p + 28, Endian.little);
    out.add(ascii.decode(zip.sublist(p + 46, p + 46 + n)));
    p += 46 + n;
  }
  return out;
}

void main() {
  test('the bytes are stable (golden SHA-256)', () async {
    final w = await sampleBundle(
      seed: [
        SeedDoc('plants/b', {'n': 2}),
        SeedDoc('plants/a', {
          'n': 1,
          'tags': ['x'],
        }),
      ],
    );
    expect(
      sha256Hex(w.bytes),
      '2a06ee04e7b52ecfa69f30fe6b7c5208c986d3d4b93a36ee2980dbccec7debb2',
    );
    final again = await sampleBundle(
      seed: [
        SeedDoc('plants/a', {
          'n': 1,
          'tags': ['x'],
        }),
        SeedDoc('plants/b', {'n': 2}),
      ],
    );
    expect(again.bytes, w.bytes);
  });

  test('entries: manifest, signature, files in byte order, seed', () async {
    final w = await sampleBundle(seed: [SeedDoc('a/b', {})]);
    expect(_names(w.bytes), [
      'manifest.json',
      'manifest.sig',
      'files/app.js',
      'files/icon.svg',
      'files/img/dot.png',
      'files/index.html',
      'files/kit/kit.js',
      'seed.jsonl',
    ]);
    expect(_names((await sampleBundle()).bytes), isNot(contains('seed.jsonl')));
  });

  test('the manifest it writes holds the hashes, kit and key', () async {
    final w = await sampleBundle();
    final m = w.manifest;
    final files = sampleFiles();
    expect(m.format, 1);
    expect(m.version, 3);
    expect(m.icon.svg, 'icon.svg');
    expect(m.icon.word, 'leaf');
    expect(m.files['img/dot.png']!.type, 'image/png');
    expect(m.files['index.html']!.sha256, sha256Hex(b('<p>hi</p>')));
    expect(
      m.contentSha256,
      contentHash({
        for (final MapEntry(:key, :value) in files.entries)
          key: (sha256: sha256Hex(value.bytes), size: value.bytes.length),
      }),
    );
    expect(
      m.kit,
      contentHash({
        'kit/kit.js': (
          sha256: sha256Hex(files['kit/kit.js']!.bytes),
          size: files['kit/kit.js']!.bytes.length,
        ),
      }),
    );
    expect(m.author.key, await publicKeyFromSeed(keySeed));
    expect(m.author.name, 'Alex');
    expect(m.seed, isNull);
    expect(m.forkedFrom, isNull);
  });

  test('no kit and no icon.svg give nulls', () async {
    final w = await sampleBundle(
      files: {'index.html': BundleFile(b('x'), type: 'text/html')},
    );
    expect(w.manifest.kit, isNull);
    expect(w.manifest.icon.svg, isNull);
  });

  test('seed lines are sorted by path, one JSON object each', () async {
    final w = await sampleBundle(
      seed: [
        SeedDoc('z/1', {'a': 'é'}),
        SeedDoc('B/1', {}),
        SeedDoc('a/1', {'x': null}),
      ],
    );
    final r = await BundleReader.read(w.bytes);
    final text = utf8.decode(BundleWriter.encodeSeed(r.seed!));
    expect(text.split('\n'), [
      '{"path":"B/1","data":{}}',
      '{"path":"a/1","data":{"x":null}}',
      '{"path":"z/1","data":{"a":"é"}}',
      '',
    ]);
    expect(w.manifest.seed!.docs, 3);
    expect(w.manifest.seed!.size, utf8.encode(text).length);
    expect(w.manifest.seed!.sha256, sha256Hex(utf8.encode(text)));
  });

  test('it reads back to the same files, seed and manifest bytes', () async {
    final w = await sampleBundle(
      seed: [
        SeedDoc('a/b', {
          'k': [1, 2],
        }),
      ],
    );
    final r = await BundleReader.read(w.bytes);
    expect(r.bytes, w.bytes);
    expect(r.manifestBytes, w.manifest.encode());
    for (final MapEntry(:key, :value) in sampleFiles().entries) {
      expect(r.files[key], value.bytes, reason: key);
    }
    expect(r.seed!.single.path, 'a/b');
    expect(r.seed!.single.data, {
      'k': [1, 2],
    });
    expect(r.iconSvg, isNotNull);
  });

  test('refuses to write what a reader would refuse', () async {
    await expectRefused(
      () => sampleBundle(
        capabilities: {
          'db': {'read': []},
        },
      ),
      'not_shareable',
    );
    await expectRefused(
      () => sampleBundle(files: {'a.html': BundleFile(b('x'), type: 't')}),
      'bad_manifest',
    );
    await expectRefused(
      () => sampleBundle(seed: [SeedDoc('data/users/u_1/p', {})]),
      'bad_seed',
    );
    await expectRefused(
      () => sampleBundle(seed: [SeedDoc('a/b', {}), SeedDoc('a/b', {})]),
      'bad_seed',
    );
  });

  test('a fork names its original', () async {
    final w = await BundleWriter.write(
      lineage: newLineage(),
      version: 1,
      title: 'Mine',
      iconWord: null,
      capabilities: {},
      files: {'index.html': BundleFile(b('x'), type: 'text/html')},
      keySeed: keySeed,
      forkedFrom: ForkedFrom(lineage: lineage, version: 3, key: 'A' * 43),
    );
    final r = await BundleReader.read(w.bytes);
    expect(r.manifest.forkedFrom!.lineage, lineage);
    expect(r.manifest.author.name, isNull);
  });
}
