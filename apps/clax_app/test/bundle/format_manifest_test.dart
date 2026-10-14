import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:clax_app/bundle/manifest.dart';
import 'package:flutter_test/flutter_test.dart';

import 'format_fixtures.dart';

Uint8List _enc(Map<String, Object?> m) => b(jsonEncode(m));

Map<String, Object?> _ok() => manifestJson({'index.html': b('<p>hi</p>')});

void _refused(Map<String, Object?> m, String code) => expect(
  () => Manifest.parse(_enc(m)),
  throwsA(isA<BundleError>().having((e) => e.code, 'code', code)),
);

void main() {
  test('round-trips through encode and parse', () {
    final m = Manifest.parse(_enc(_ok()));
    expect(m.lineage, lineage);
    expect(m.files['index.html']!.size, 9);
    final again = Manifest.parse(m.encode());
    expect(again.encode(), m.encode());
    expect(jsonDecode(utf8.decode(again.encode())), _ok());
  });

  test('encode: required-list key order, two spaces, LF, final newline', () {
    final m = Manifest.parse(
      _enc(
        _ok()
          ..['seed'] = {
            'sha256': '0' * 64, 'size': 5, 'docs': 1, //
          },
      ),
    );
    final s = utf8.decode(m.encode());
    expect(s, endsWith('}\n'));
    expect(s, isNot(contains('\r')));
    expect(s, startsWith('{\n  "format": 1,\n  "lineage": '));
    final order = [
      'format', 'lineage', 'version', 'title', 'icon', 'capabilities', 'kit',
      'files', 'content_sha256', 'seed', 'author', 'forked_from', //
    ];
    final at = [for (final k in order) s.indexOf('\n  "$k": ')];
    expect(at, everyElement(greaterThan(0)));
    expect([...at]..sort(), at);
    expect(
      s.indexOf('"sha256"', s.indexOf('"seed"')),
      lessThan(s.indexOf('"docs"', s.indexOf('"seed"'))),
    );
  });

  test('format over 1 is too_new, before anything else is checked', () {
    _refused({'format': 2, 'whatever': true}, 'too_new');
    _refused(_ok()..['format'] = 0, 'bad_manifest');
    _refused(_ok()..['format'] = 1.0, 'bad_manifest');
  });

  test('refuses a manifest that breaks the schema', () {
    final cases = <String, void Function(Map<String, Object?>)>{
      'unknown key': (m) => m['extra'] = 1,
      'missing key': (m) => m.remove('kit'),
      'bad lineage': (m) => m['lineage'] = 'l_ABC',
      'version 0': (m) => m['version'] = 0,
      'version too big': (m) => m['version'] = 2147483648,
      'empty title': (m) => m['title'] = '',
      'long title': (m) => m['title'] = 'x' * 201,
      'icon svg other name': (m) => m['icon'] = {'svg': 'x.svg', 'word': null},
      'icon word bad': (m) => m['icon'] = {'svg': null, 'word': 'Leaf'},
      'icon extra key': (m) => m['icon'] = {'svg': null, 'word': null, 'x': 1},
      'capabilities not object': (m) => m['capabilities'] = [],
      'kit bad': (m) => m['kit'] = 'sha256:XYZ',
      'no files': (m) => m['files'] = {},
      'file extra key': (m) => (m['files']! as Map)['index.html'] = {
        'sha256': '0' * 64, 'size': 1, 'type': 't', 'x': 1, //
      },
      'file size over 16 MiB': (m) => (m['files']! as Map)['index.html'] = {
        'sha256': '0' * 64, 'size': 16777217, 'type': 't', //
      },
      'file type too long': (m) => (m['files']! as Map)['index.html'] = {
        'sha256': '0' * 64, 'size': 1, 'type': 't' * 101, //
      },
      'upper-case file hash': (m) => (m['files']! as Map)['index.html'] = {
        'sha256': 'A' * 64, 'size': 1, 'type': 't', //
      },
      'content hash bad': (m) => m['content_sha256'] = '0' * 64,
      'seed size 0': (m) =>
          m['seed'] = {'sha256': '0' * 64, 'size': 0, 'docs': 1},
      'seed docs over 2000': (m) =>
          m['seed'] = {'sha256': '0' * 64, 'size': 9, 'docs': 2001},
      'author key short': (m) => m['author'] = {'key': 'A' * 42, 'name': null},
      'author name empty': (m) => m['author'] = {'key': 'A' * 43, 'name': ''},
      'author name long': (m) =>
          m['author'] = {'key': 'A' * 43, 'name': 'n' * 65},
      'forked_from missing key': (m) =>
          m['forked_from'] = {'lineage': lineage, 'version': 1},
      'no index.html': (m) => m['files'] = {
        'main.html': {'sha256': '0' * 64, 'size': 1, 'type': 't'},
      },
      'icon.svg named but absent': (m) =>
          m['icon'] = {'svg': 'icon.svg', 'word': null},
      'case-folded duplicate path': (m) => (m['files']! as Map)['INDEX.html'] =
          {'sha256': '0' * 64, 'size': 1, 'type': 't'},
      'over 64 MiB in all': (m) => m['files'] = {
        'index.html': {'sha256': '0' * 64, 'size': 1, 'type': 't'},
        for (var i = 0; i < 4; i++)
          'f$i': {'sha256': '0' * 64, 'size': 16777216, 'type': 't'},
      },
    };
    for (final MapEntry(:key, :value) in cases.entries) {
      final m = _ok();
      value(m);
      expect(
        () => Manifest.parse(_enc(m)),
        throwsA(
          isA<BundleError>().having((e) => e.code, 'code', 'bad_manifest'),
        ),
        reason: key,
      );
    }
  });

  test('accepts the schema\'s optional shapes', () {
    final m = _ok()
      ..['icon'] = {'svg': null, 'word': 'a-1'}
      ..['kit'] = 'sha256:${'a' * 64}'
      ..['seed'] = {'sha256': 'b' * 64, 'size': 1, 'docs': 1}
      ..['author'] = {'key': 'A' * 43, 'name': 'Alex'}
      ..['forked_from'] = {'lineage': lineage, 'version': 7, 'key': '_' * 43}
      ..['title'] = '\u{1F331}' * 200;
    final p = Manifest.parse(_enc(m));
    expect(p.icon.word, 'a-1');
    expect(p.seed!.docs, 1);
    expect(p.forkedFrom!.version, 7);
    expect(p.author.name, 'Alex');
  });

  test('bundle paths follow §6.3', () {
    for (final ok in ['index.html', 'a/b.c', '_x', 'tests/a.test.js', 'a..b']) {
      expect(isBundlePath(ok), isTrue, reason: ok);
    }
    for (final bad in [
      '../x', '/x', r'a\b', 'a/./b', 'a/../b', '.', '..', 'a//b', 'a/',
      '.hidden', 'a/.', 'a b', 'a\x00b', 'é', 'x' * 201, '', //
    ]) {
      expect(isBundlePath(bad), isFalse, reason: bad);
      final m = _ok();
      (m['files']! as Map)[bad] = {'sha256': '0' * 64, 'size': 1, 'type': 't'};
      _refused(m, 'bad_manifest');
    }
  });

  test('db.read or db.share is not_shareable', () {
    _refused(
      _ok()
        ..['capabilities'] = {
          'db': {'read': []},
        },
      'not_shareable',
    );
    _refused(
      _ok()
        ..['capabilities'] = {
          'db': {'share': []},
        },
      'not_shareable',
    );
    Manifest.parse(
      _enc(
        _ok()
          ..['capabilities'] = {
            'db': {'rules': []},
          },
      ),
    );
  });

  test('not UTF-8, a BOM or not JSON is bad_manifest', () {
    for (final bytes in [
      Uint8List.fromList([0xff, 0xfe]),
      Uint8List.fromList([0xef, 0xbb, 0xbf, ...utf8.encode('{}')]),
      b('[1]'),
      b('{'),
    ]) {
      expect(
        () => Manifest.parse(bytes),
        throwsA(
          isA<BundleError>().having((e) => e.code, 'code', 'bad_manifest'),
        ),
      );
    }
  });

  test('a new lineage is l_ and 26 lower-case base32 characters', () {
    final l = newLineage(Random(1));
    expect(l, matches(RegExp(r'^l_[a-z2-7]{26}$')));
    expect(newLineage(Random(1)), l);
    expect(newLineage(Random(2)), isNot(l));
  });

  test('base32 is RFC 4648 without padding', () {
    expect(base32(utf8.encode('foobar')), 'mzxw6ytboi');
    expect(base32(utf8.encode('foobar'), upper: true), 'MZXW6YTBOI');
    expect(base32(utf8.encode('f')), 'my');
  });

  test('title and author name refuse hidden and direction characters', () {
    for (final t in [
      'Bank\u202Egnp.exe', 'a\nb', 'a\u0000', 'a\u0085', 'a\u200Bb',
      'a\u2066b', 'a\u061Cb', 'a\uFEFF', 'a\u2028b', //
    ]) {
      _refused(_ok()..['title'] = t, 'bad_manifest');
      _refused(
        _ok()..['author'] = {'key': 'A' * 43, 'name': t},
        'bad_manifest',
      );
    }
    // Joiners inside emoji and scripts stay allowed.
    final family = '\u{1F468}\u200D\u{1F469}\u200D\u{1F467} m\u200Cn';
    final m = Manifest.parse(
      _enc(
        _ok()
          ..['title'] = family
          ..['author'] = {'key': 'A' * 43, 'name': 'Zoë'},
      ),
    );
    expect(m.title, family);
  });
}
