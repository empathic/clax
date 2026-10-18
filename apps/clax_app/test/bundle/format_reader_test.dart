import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:archive/archive.dart' show Deflate;
import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/manifest.dart' show maxFilesBytes;
import 'package:clax_app/bundle/signing.dart';
import 'package:flutter_test/flutter_test.dart';

import 'format_fixtures.dart';

final _index = b('<p>hi</p>');

Map<String, Uint8List> _files() => {'index.html': _index};

/// Offset of the central directory entry named [name] in [zip].
int _central(Uint8List zip, String name) {
  final d = ByteData.sublistView(zip);
  final eocd = zip.length - 22;
  var p = d.getUint32(eocd + 16, Endian.little);
  while (true) {
    final n = d.getUint16(p + 28, Endian.little);
    if (ascii.decode(zip.sublist(p + 46, p + 46 + n)) == name) return p;
    p += 46 + n;
  }
}

int _local(Uint8List zip, String name) =>
    ByteData.sublistView(zip)
        .getUint32(_central(zip, name) + 42, Endian.little);

void _u16(Uint8List z, int at, int v) =>
    ByteData.sublistView(z).setUint16(at, v, Endian.little);

void _u32(Uint8List z, int at, int v) =>
    ByteData.sublistView(z).setUint32(at, v, Endian.little);

void main() {
  test('reads a hand-made bundle, stored or deflated', () async {
    for (final deflate in [false, true]) {
      final r = await BundleReader.read(
        await craft(manifestJson(_files()), _files(), deflate: deflate),
      );
      expect(r.files['index.html'], _index);
      expect(r.seed, isNull);
      expect(r.iconSvg, isNull);
    }
  });

  test('a file over 32 MiB is too_large before it is parsed', () async {
    await expectReadRefused(Uint8List(32 * 1024 * 1024 + 1), 'too_large');
  });

  test('not a zip is not_a_bundle', () async {
    await expectReadRefused(b('hello'), 'not_a_bundle');
    await expectReadRefused(Uint8List(0), 'not_a_bundle');
  });

  test('a tampered file byte is bad_file', () async {
    final z = await craft(manifestJson(_files()), _files());
    final at = _local(z, 'files/index.html') + 30 + 'files/index.html'.length;
    z[at] ^= 1;
    await expectReadRefused(z, 'bad_file');
  });

  test('a tampered manifest is bad_signature', () async {
    final w = await sampleBundle();
    // The writer deflates the manifest; zip it again with one byte changed.
    final t = utf8
        .decode(w.manifest.encode())
        .replaceFirst('"Plant log"', '"Plant lag"');
    final sig = await signManifest(w.manifest.encode(), keySeed);
    final bad = BundleWriter.zip([
      ZipEntry('manifest.json', b(t)),
      ZipEntry('manifest.sig', b(sig)),
      for (final MapEntry(:key, :value) in sampleFiles().entries)
        ZipEntry('files/$key', value.bytes),
    ]);
    await expectReadRefused(bad, 'bad_signature');
  });

  test('a signature by another key is bad_signature', () async {
    final other = Uint8List.fromList(List.generate(32, (i) => i));
    final m = manifestJson(_files())
      ..['author'] = {'key': await publicKeyFromSeed(keySeed), 'name': null};
    final bytes = b('${const JsonEncoder.withIndent('  ').convert(m)}\n');
    final z = await craft(
      m,
      _files(),
      keepKey: true,
      signature: await signManifest(bytes, other),
    );
    await expectReadRefused(z, 'bad_signature');
  });

  test('a malformed or missing signature entry is refused', () async {
    await expectReadRefused(
      await craft(manifestJson(_files()), _files(), signature: 'x'),
      'bad_signature',
    );
    final m = b('${jsonEncode(manifestJson(_files()))}\n');
    await expectReadRefused(
      BundleWriter.zip([
        ZipEntry('manifest.json', m),
        ZipEntry('files/index.html', _index),
      ]),
      'not_a_bundle',
    );
  });

  test('a missing file entry is bad_file', () async {
    final m = manifestJson({..._files(), 'a.js': b('x')});
    await expectReadRefused(await craft(m, _files()), 'bad_file');
  });

  test('an extra file entry is bad_file', () async {
    await expectReadRefused(
      await craft(manifestJson(_files()), {..._files(), 'a.js': b('x')}),
      'bad_file',
    );
  });

  test('an entry outside the layout is not_a_bundle', () async {
    for (final name in [
      'other.txt',
      'files/',
      'files/../x',
      'files//x',
      r'files/a\b',
      'files/a/./b',
      'files/.x',
      'seed.jsonl/x',
      'Manifest.json',
    ]) {
      await expectReadRefused(
        await craft(
          manifestJson(_files()),
          _files(),
          extra: [ZipEntry(name, b('x'))],
        ),
        'not_a_bundle',
      );
    }
  });

  test('a duplicate entry name is not_a_bundle', () async {
    await expectReadRefused(
      await craft(
        manifestJson(_files()),
        _files(),
        extra: [ZipEntry('files/index.html', _index)],
      ),
      'not_a_bundle',
    );
  });

  test('a case-folded duplicate path is bad_manifest', () async {
    final files = {..._files(), 'A.js': b('1'), 'a.js': b('2')};
    await expectReadRefused(
      await craft(manifestJson(files), files),
      'bad_manifest',
    );
  });

  test('more than 515 entries is too_large', () async {
    await expectReadRefused(
      await craft(
        manifestJson(_files()),
        _files(),
        extra: [for (var i = 0; i < 514; i++) ZipEntry('files/f$i', b('x'))],
      ),
      'too_large',
    );
  });

  test('a manifest over 256 KiB is too_large', () async {
    final m = manifestJson(_files())..['title'] = 'T';
    (m['capabilities']! as Map)['pad'] = 'x' * (256 * 1024);
    await expectReadRefused(
      await craft(m, _files(), deflate: true),
      'too_large',
    );
  });

  test('a header that understates a size is caught by the counter', () async {
    // The manifest says 9 bytes; the entry inflates to 1 MiB and its
    // headers claim 9.
    final big = Uint8List(1024 * 1024);
    final z = await craft(manifestJson(_files()), {
      'index.html': big,
    }, deflate: true);
    _u32(z, _central(z, 'files/index.html') + 24, 9);
    _u32(z, _local(z, 'files/index.html') + 22, 9);
    await expectReadRefused(z, 'bad_file');
  });

  test('inflate stops one byte past the cap, whatever the stream holds', () {
    final bomb = Deflate(Uint8List(4 * 1024 * 1024)).getBytes();
    expect(inflateAtMost(bomb, 1000), isNull);
    expect(inflateAtMost(bomb, 4 * 1024 * 1024), hasLength(4 * 1024 * 1024));
    expect(inflateAtMost(Uint8List.fromList([0xff, 0xff]), 10), isNull);
  });

  test('a stream whose last code needs fewer bits than the longest code inflates whole', () {
    // A fixed Huffman stream of 13 "x"s ends with bits to spare for its
    // end-of-block code but not for a 9-bit lookahead.
    for (final s in ['x' * 13, 'word0 word1 ']) {
      final data = utf8.encode(s);
      final deflated = Deflate(data).getBytes();
      expect(inflateAtMost(Uint8List.fromList(deflated), data.length), data, reason: s);
    }
  });

  test('compression method 12 is not_a_bundle', () async {
    final z = await craft(manifestJson(_files()), _files());
    _u16(z, _central(z, 'files/index.html') + 10, 12);
    _u16(z, _local(z, 'files/index.html') + 8, 12);
    await expectReadRefused(z, 'not_a_bundle');
  });

  test('the encryption flag is not_a_bundle', () async {
    final z = await craft(manifestJson(_files()), _files());
    _u16(z, _central(z, 'files/index.html') + 8, 1);
    _u16(z, _local(z, 'files/index.html') + 6, 1);
    await expectReadRefused(z, 'not_a_bundle');
  });

  test('zip64 is not_a_bundle', () async {
    final z = await craft(manifestJson(_files()), _files());
    _u32(z, _central(z, 'files/index.html') + 20, 0xffffffff);
    await expectReadRefused(z, 'not_a_bundle');
    final y = await craft(manifestJson(_files()), _files());
    _u16(y, y.length - 22 + 10, 0xffff);
    await expectReadRefused(y, 'not_a_bundle');
  });

  test(
    'a local name that differs from the central one is not_a_bundle',
    () async {
      final z = await craft(manifestJson(_files()), _files());
      final at = _local(z, 'files/index.html') + 30;
      z[at + 'files/'.length] = 'j'.codeUnitAt(0);
      await expectReadRefused(z, 'not_a_bundle');
    },
  );

  test('format 2 is too_new', () async {
    await expectReadRefused(
      await craft(manifestJson(_files())..['format'] = 2, _files()),
      'too_new',
    );
  });

  test('an unknown manifest key is bad_manifest', () async {
    await expectReadRefused(
      await craft(manifestJson(_files())..['extra'] = 1, _files()),
      'bad_manifest',
    );
  });

  test('db.read present is not_shareable', () async {
    await expectReadRefused(
      await craft(
        manifestJson(_files())
          ..['capabilities'] = {
            'db': {
              'read': ['a_x'],
            },
          },
        _files(),
      ),
      'not_shareable',
    );
  });

  test('a content hash that does not match the files is bad_file', () async {
    await expectReadRefused(
      await craft(
        manifestJson(_files())..['content_sha256'] = 'sha256:${'0' * 64}',
        _files(),
      ),
      'bad_file',
    );
  });

  group('seed', () {
    Future<Uint8List> seeded(String text, {int? docs, int? size}) {
      final bytes = b(text);
      return craft(
        manifestJson(
          _files(),
          seed: {
            'sha256': sha256Hex(bytes),
            'size': size ?? bytes.length,
            'docs': docs ?? max(1, '\n'.allMatches(text).length),
          },
        ),
        _files(),
        seedBytes: bytes,
      );
    }

    test('a valid seed reads', () async {
      final r = await BundleReader.read(
        await seeded(
          '{"path":"a/1","data":{"x":1}}\n{"data":{},"path":"b/2"}\n',
        ),
      );
      expect(r.seed!.map((d) => d.path), ['a/1', 'b/2']);
    });

    test('present without a manifest seed, or absent with one', () async {
      await expectReadRefused(
        await craft(manifestJson(_files()), _files(), seedBytes: b('x')),
        'bad_seed',
      );
      final bytes = b('{"path":"a/1","data":{}}\n');
      await expectReadRefused(
        await craft(
          manifestJson(
            _files(),
            seed: {
              'sha256': sha256Hex(bytes), 'size': bytes.length, 'docs': 1, //
            },
          ),
          _files(),
        ),
        'bad_seed',
      );
    });

    test('bad lines are bad_seed', () async {
      for (final text in [
        '{"path":"a/1","data":{}}', // no final newline
        '{"path":"a/1","data":[]}\n',
        '{"path":"a/1","data":{},"x":1}\n',
        '{"path":"a","data":{}}\n',
        '{"path":"a/../b","data":{}}\n',
        '{"path":"a/1","data":{}}\n{"path":"a/1","data":{}}\n',
        '{"path":"data/users/u_1/p","data":{}}\n',
        'not json\n',
        '\n',
      ]) {
        await expectReadRefused(await seeded(text), 'bad_seed');
      }
    });

    test(
      'a line count other than docs, or a wrong size, is bad_seed',
      () async {
        await expectReadRefused(
          await seeded('{"path":"a/1","data":{}}\n', docs: 2),
          'bad_seed',
        );
        await expectReadRefused(
          await seeded('{"path":"a/1","data":{}}\n', size: 5),
          'bad_seed',
        );
      },
    );

    test('over 2,000 lines is refused', () async {
      final text = StringBuffer();
      for (var i = 0; i < 2001; i++) {
        text.write('{"path":"a/$i","data":{}}\n');
      }
      // The manifest schema caps docs at 2,000, so the bundle fails there.
      await expectReadRefused(await seeded(text.toString()), 'bad_manifest');
      // With docs claiming 2,000, the line count is caught on the bytes.
      await expectReadRefused(
        await seeded(text.toString(), docs: 2000),
        'bad_seed',
      );
    });

    test('data/users itself is a document of the owner, so it reads', () async {
      final r = await BundleReader.read(
        await seeded('{"path":"data/users","data":{}}\n'),
      );
      expect(r.seed!.single.path, 'data/users');
    });
  });

  test('an icon.svg that cannot be drawn reads, with no icon', () async {
    final files = {..._files(), 'icon.svg': b('<svg><oops')};
    final m = manifestJson(files)..['icon'] = {'svg': 'icon.svg', 'word': null};
    final r = await BundleReader.read(await craft(m, files));
    expect(r.iconSvg, isNull);
    final good = {..._files(), 'icon.svg': b(iconSvg)};
    final g = manifestJson(good)..['icon'] = {'svg': 'icon.svg', 'word': null};
    expect((await BundleReader.read(await craft(g, good))).iconSvg, isNotNull);
  });

  group('work bounds', () {
    test('many empty dynamic blocks are refused, not inflated', () async {
      final bomb = _emptyBlocks(20000);
      expect(bomb.length, lessThan(256 * 1024));
      // Each block builds a 2^15-entry table and yields nothing: refused
      // once the block count passes the cap for the declared size.
      expect(inflateAtMost(bomb, maxFilesBytes), isNull);
      // The block form itself is valid deflate: one of them inflates.
      expect(inflateAtMost(_emptyBlocks(1), 100), isEmpty);
      // As manifest.json, before any signature check.
      await expectReadRefused(
        _rawDeflatedZip('manifest.json', bomb),
        'too_large',
      );
    });

    test('a compressed entry much larger than its declared size is refused '
        'unread', () async {
      final z = await craft(manifestJson(_files()), {
        'index.html': Uint8List.fromList(List.generate(4096, (i) => i * 7)),
      });
      await expectReadRefused(z, 'bad_file');
    });

    test('a file over the inline size reads on another isolate', () async {
      final big = Uint8List.fromList(
        List.generate(200 * 1024, (i) => (i * 2654435761) >> 13 & 0xff),
      );
      final files = {..._files(), 'big.bin': big};
      final z = await craft(manifestJson(files), files);
      expect(z.length, greaterThan(inlineReadBytes));
      expect((await BundleReader.read(z)).files['big.bin'], big);
      z[_local(z, 'files/big.bin') + 30 + 'files/big.bin'.length] ^= 1;
      await expectReadRefused(z, 'bad_file');
    });
  });

  group('bytes outside the entries', () {
    Future<void> junkAt(int Function(Uint8List z) where) async {
      final z = await craft(manifestJson(_files()), _files());
      expect(await BundleReader.read(z), isNotNull);
      await expectReadRefused(_insert(z, where(z), 4096), 'not_a_bundle');
    }

    test('before the first entry', () => junkAt((_) => 0));
    test('between entries', () => junkAt((z) => _local(z, 'manifest.sig')));
    test('after the last entry', () => junkAt(_cdOffset));
    test('after the end record', () async {
      final z = await craft(manifestJson(_files()), _files());
      await expectReadRefused(Uint8List.fromList([...z, 0]), 'not_a_bundle');
    });
  });

  group('JSON depth', () {
    Object? nest(int depth) => depth == 1 ? <Object?>[] : [nest(depth - 1)];

    test('capabilities deeper than 64 levels is bad_manifest', () async {
      final ok = manifestJson(_files())..['capabilities'] = {'x': nest(63)};
      await BundleReader.read(await craft(ok, _files()));
      await expectReadRefused(
        await craft(
          manifestJson(_files())..['capabilities'] = {'x': nest(64)},
          _files(),
        ),
        'bad_manifest',
      );
    });

    test('a seed document deeper than 64 levels is bad_seed', () async {
      Future<Uint8List> seeded(String data) {
        final bytes = b('{"path":"a/1","data":$data}\n');
        return craft(
          manifestJson(
            _files(),
            seed: {'sha256': sha256Hex(bytes), 'size': bytes.length, 'docs': 1},
          ),
          _files(),
          seedBytes: bytes,
        );
      }

      await BundleReader.read(await seeded('{"x":${'[' * 63}${']' * 63}}'));
      await expectReadRefused(
        await seeded('{"x":${'[' * 64}${']' * 64}}'),
        'bad_seed',
      );
      // Far deeper than any stack: refused, not a StackOverflowError.
      await expectReadRefused(
        await seeded('{"x":${'[' * 100000}${']' * 100000}}'),
        'bad_seed',
      );
    });
  });
}

int _cdOffset(Uint8List z) =>
    ByteData.sublistView(z).getUint32(z.length - 22 + 16, Endian.little);

/// [z] with [n] zero bytes inserted at [at] and every offset past it moved,
/// so the zip is otherwise consistent.
Uint8List _insert(Uint8List z, int at, int n) {
  final out = Uint8List.fromList([
    ...z.sublist(0, at),
    ...Uint8List(n),
    ...z.sublist(at),
  ]);
  final d = ByteData.sublistView(out);
  final eocd = out.length - 22;
  final count = d.getUint16(eocd + 10, Endian.little);
  final cd = d.getUint32(eocd + 16, Endian.little);
  final newCd = cd >= at ? cd + n : cd;
  d.setUint32(eocd + 16, newCd, Endian.little);
  var p = newCd;
  for (var i = 0; i < count; i++) {
    final local = d.getUint32(p + 42, Endian.little);
    if (local >= at) d.setUint32(p + 42, local + n, Endian.little);
    p += 46 + d.getUint16(p + 28, Endian.little);
  }
  return out;
}

/// A one-entry zip whose data is [deflated] as given, method 8.
Uint8List _rawDeflatedZip(String name, Uint8List deflated) {
  final z = BundleWriter.zip([ZipEntry(name, deflated)]);
  _u16(z, _central(z, name) + 10, 8);
  _u16(z, _local(z, name) + 8, 8);
  return z;
}

/// [count] dynamic deflate blocks that each declare a 15-bit literal code
/// and a 1-bit end of block and hold only the end of block, then a final
/// empty stored block.
Uint8List _emptyBlocks(int count) {
  final w = _Bits();
  const order = [
    16,
    17,
    18,
    0,
    8,
    7,
    9,
    6,
    10,
    5,
    11,
    4,
    12,
    3,
    13,
    2,
    14,
    1,
    15,
  ];
  for (var n = 0; n < count; n++) {
    w.bits(0, 1); // not final
    w.bits(2, 2); // dynamic
    w.bits(0, 5); // 257 literal/length codes
    w.bits(0, 5); // 1 distance code
    w.bits(15, 4); // 19 code length codes
    for (final sym in order) {
      w.bits(const {0, 1, 15, 18}.contains(sym) ? 2 : 0, 3);
    }
    // Code length codes (2 bits each, canonical): 0->00 1->01 15->10 18->11.
    w.code(2, 2); // literal 0: length 15
    w.code(3, 2); // 18: 138 zeros
    w.bits(127, 7);
    w.code(3, 2); // 18: 117 zeros (literals 1 to 255)
    w.bits(106, 7);
    w.code(1, 2); // 256 (end of block): length 1
    w.code(0, 2); // distance 0: length 0
    w.code(0, 1); // end of block
  }
  w.bits(1, 1); // final
  w.bits(0, 2); // stored
  w.align();
  w.bytes([0, 0, 0xff, 0xff]);
  return w.done();
}

class _Bits {
  final _out = <int>[];
  var _acc = 0;
  var _n = 0;

  void bits(int v, int n) {
    for (var i = 0; i < n; i++) {
      _acc |= ((v >> i) & 1) << _n;
      if (++_n == 8) {
        _out.add(_acc);
        _acc = 0;
        _n = 0;
      }
    }
  }

  /// A Huffman code, most significant bit first.
  void code(int v, int n) {
    for (var i = n - 1; i >= 0; i--) {
      bits((v >> i) & 1, 1);
    }
  }

  void align() {
    if (_n > 0) bits(0, 8 - _n);
  }

  void bytes(List<int> b) => _out.addAll(b);

  Uint8List done() => Uint8List.fromList(_out);
}
