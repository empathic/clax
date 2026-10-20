import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/icon_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import '../support/paths.dart';

/// The icon rule vectors the core's publish check runs too
/// (crates/clax-core/src/icon.rs): each file's answer in expect.json, a
/// problem's code and sentence or none, and whether [cleanIcon] draws it.
/// A `not_xml` sentence is compared up to the parser's own account, which
/// the two parsers word differently. mutations.json holds a seeded set of
/// mutations of the vectors with the core's answer to each
/// (icon::tests::the_mutations_file_holds_the_cores_answers), and
/// [iconProblem] must give the same.
void main() {
  final dir = appPath('../../crates/clax-core/tests/fixtures/icons');
  final vectors =
      jsonDecode(File('$dir/expect.json').readAsStringSync())
          as Map<String, dynamic>;

  test('every fixture has an answer', () {
    final files = Directory(dir)
        .listSync()
        .map((f) => f.uri.pathSegments.last)
        .where((n) => n.endsWith('.svg'))
        .toSet();
    expect(files, vectors.keys.toSet());
  });

  // The icons the app's agent wrote (test/fixtures/icons, which the
  // screenshots draw) are among the vectors as ok-sample-<name>.svg, byte
  // for byte, so a stricter core rule cannot refuse them unseen.
  test('the app\'s sample icons are vectors', () {
    final samples = Directory(appPath('test/fixtures/icons'))
        .listSync()
        .whereType<File>()
        .where((f) => f.path.endsWith('.svg'))
        .toList();
    expect(samples, hasLength(6));
    for (final f in samples) {
      final name = 'ok-sample-${f.uri.pathSegments.last}';
      expect(vectors[name], {'problem': null, 'clean': true}, reason: name);
      expect(
        File('$dir/$name').readAsBytesSync(),
        f.readAsBytesSync(),
        reason: name,
      );
    }
  });

  // The problems cleanIcon drops rather than refuses.
  const cleanable = {
    'element',
    'event_attribute',
    'external_href',
    'filter',
    'external_url',
    'style',
  };

  for (final MapEntry(key: name, :value) in vectors.entries) {
    test(name, () {
      final svg = _read('$dir/$name');
      final want = value['problem'] as Map<String, dynamic>?;
      final got = iconProblem(svg);
      if (want == null) {
        expect(got, isNull);
      } else {
        expect(got?.code, want['code']);
        final message = want['message'] as String;
        expect(
          got?.message,
          want['code'] == 'not_xml' ? startsWith(message) : message,
        );
      }
      final cleaned = cleanIcon(svg);
      expect(cleaned != null, value['clean'], reason: 'cleanIcon');
      if (want == null || cleanable.contains(want['code'])) {
        expect(iconProblem(cleaned!), isNull, reason: cleaned);
      }
    });
  }

  test('the app agrees with the core on every mutation', () {
    final mutations =
        jsonDecode(File('$dir/mutations.json').readAsStringSync()) as List;
    expect(mutations, hasLength(600));
    final wrong = <String>[];
    for (final (i, m) in mutations.indexed) {
      final svg = m['svg'] as String;
      final got = iconProblem(svg);
      final want = m['message'] as String?;
      if (got?.code != m['code'] || (want != null && got?.message != want)) {
        wrong.add(
          '#$i (from ${m['from']}): the core says ${m['code']} $want, the '
          'app ${got?.code} ${got?.message}\n  ${jsonEncode(svg)}',
        );
      }
      // cleanIcon reads anything without throwing.
      cleanIcon(svg);
    }
    expect(wrong, isEmpty, reason: wrong.take(5).join('\n'));
  });
}

/// [path]'s text with its byte order mark, which `utf8.decode` drops: the
/// core reads the bytes as they are.
String _read(String path) {
  final bytes = File(path).readAsBytesSync();
  final text = utf8.decode(bytes);
  final bom =
      bytes.length >= 3 &&
      bytes[0] == 0xEF &&
      bytes[1] == 0xBB &&
      bytes[2] == 0xBF;
  return bom ? '\ufeff$text' : text;
}
