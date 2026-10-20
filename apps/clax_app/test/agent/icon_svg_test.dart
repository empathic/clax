import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/icon_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import '../support/paths.dart';

const _ok =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
    '<defs><linearGradient id="g"><stop offset="0" stop-color="#f80"/>'
    '</linearGradient></defs>'
    '<rect width="1024" height="1024" fill="url(#g)"/>'
    '<path d="M200 700 L800 300" stroke="#fff" stroke-width="64"/></svg>';

String _with(String inner, {String box = '0 0 1024 1024'}) =>
    '<svg xmlns="http://www.w3.org/2000/svg" '
    'xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="$box">'
    '<rect width="1024" height="1024" fill="#123"/>$inner</svg>';

/// [iconProblem]'s sentence.
String? problem(String svg) => iconProblem(svg)?.message;

void main() {
  group('iconProblem', () {
    test('passes a plain square icon and every sample fixture', () {
      expect(problem(_ok), isNull);
      for (final f in Directory(appPath('test/fixtures/icons')).listSync()) {
        expect(
          problem(File(f.path).readAsStringSync()),
          isNull,
          reason: f.path,
        );
      }
    });

    test('refuses a script, a foreignObject, an image and a filter', () {
      expect(
        problem(_with('<script>alert(1)</script>')),
        contains('<script>'),
      );
      expect(
        problem(_with('<foreignObject><p>x</p></foreignObject>')),
        contains('<foreignObject>'),
      );
      expect(
        problem(_with('<image href="#a" width="9" height="9"/>')),
        contains('<image>'),
      );
      expect(problem(_with('<filter id="f"/>')), contains('<filter>'));
      expect(problem(_with('<g filter="url(#f)"/>')), contains('filters'));
      expect(problem(_with('<style>*{}</style>')), contains('<style>'));
      expect(
        problem(_with('<rect width="1" height="1" onclick="x()"/>')),
        contains('onclick'),
      );
    });

    test('refuses an external href or url()', () {
      expect(
        problem(_with('<use xlink:href="https://evil.example/a.svg#x"/>')),
        contains('outside the file'),
      );
      expect(
        problem(_with('<use href="other.svg#x"/>')),
        contains('outside the file'),
      );
      expect(
        problem(_with('<rect width="1" height="1" fill="url(a.svg#g)"/>')),
        contains('outside the file'),
      );
      expect(problem(_with('<use href="#x"/>')), isNull);
    });

    test('refuses <use> that expands too far or nests too deep', () {
      // Each level uses the one below ten times: 10 + 100 + … copies.
      final bomb = _with(
        '<defs><rect id="a" width="1" height="1"/>'
        '<g id="b">${'<use href="#a"/>' * 10}</g>'
        '<g id="c">${'<use href="#b"/>' * 10}</g></defs>'
        '<use href="#c"/>',
      );
      expect(problem(bomb), contains('expand'));
      expect(cleanIcon(bomb), isNull);
      final cycle = _with(
        '<defs><g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g>'
        '</defs><use href="#a"/>',
      );
      expect(problem(cycle), contains('nest'));
      expect(
        problem(
          _with(
            '<defs><circle id="d" r="9"/></defs>'
            '${'<use href="#d"/>' * 8}',
          ),
        ),
        isNull,
      );
    });

    test('refuses a huge, malformed, non-svg or non-square file', () {
      final huge = _with('<path d="${'M0 0 L1 1 ' * 4000}"/>');
      expect(problem(huge), contains('at most 32 KB'));
      expect(problem('<svg viewBox="0 0 1 1">'), contains('well-formed'));
      expect(problem('<g viewBox="0 0 10 10"/>'), contains('root element'));
      expect(
        problem(_with('', box: '0 0 1024 512')),
        contains('not square'),
      );
      expect(
        problem('<svg xmlns="http://www.w3.org/2000/svg"/>'),
        contains('square viewBox'),
      );
      expect(
        problem('<!DOCTYPE svg [<!ENTITY a "b">]>${_with('')}'),
        contains('DOCTYPE'),
      );
    });
  });

  group('cleanIcon', () {
    test('drops what the subset leaves out and keeps the rest', () {
      final out = cleanIcon(
        _with(
          '<script>alert(1)</script><foreignObject/>'
          '<image href="https://x.example/a.png"/>'
          '<circle r="9" onload="x()" fill="#fff" filter="url(#f)"/>'
          '<use href="https://x.example/a.svg"/>',
        ),
      )!;
      expect(out, isNot(contains('script')));
      expect(out, isNot(contains('foreignObject')));
      expect(out, isNot(contains('image')));
      expect(out, isNot(contains('onload')));
      expect(out, isNot(contains('filter')));
      expect(out, isNot(contains('x.example')));
      expect(out, contains('<circle r="9" fill="#fff"/>'));
      expect(out, contains('fill="#123"'));
    });

    test('is null for what cannot be drawn', () {
      expect(cleanIcon('not xml'), isNull);
      expect(cleanIcon(_with('', box: '0 0 10 20')), isNull);
      expect(cleanIcon('<g viewBox="0 0 1 1"/>'), isNull);
    });
  });

  group('iconBytesProblem', () {
    // As the core's problem: size first, then UTF-8, then the rules.
    test('reads bytes as the core does', () {
      final cafe = latin1.encode(_with('<title>Caf\u00e9</title>'));
      expect(iconBytesProblem(utf8.encode(_ok)), isNull);
      expect(iconBytesProblem(cafe), (
        code: 'not_xml',
        message: 'icon.svg is not well-formed XML: it is not UTF-8.',
      ));
      final big = [...cafe, ...List.filled(maxIconBytes, 0x20)];
      expect(iconBytesProblem(big)!.code, 'too_large');
      expect(iconText(cafe), _with('<title>Caf\u00e9</title>'));
      expect(iconText(utf8.encode(_ok)), _ok);
    });
  });
}
