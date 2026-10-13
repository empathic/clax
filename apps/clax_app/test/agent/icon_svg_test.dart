import 'dart:io';

import 'package:clax_app/agent/icon_svg.dart';
import 'package:flutter_test/flutter_test.dart';

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

void main() {
  group('iconProblem', () {
    test('passes a plain square icon and every sample fixture', () {
      expect(iconProblem(_ok), isNull);
      for (final f in Directory('test/fixtures/icons').listSync()) {
        expect(
          iconProblem(File(f.path).readAsStringSync()),
          isNull,
          reason: f.path,
        );
      }
    });

    test('refuses a script, a foreignObject, an image and a filter', () {
      expect(
        iconProblem(_with('<script>alert(1)</script>')),
        contains('<script>'),
      );
      expect(
        iconProblem(_with('<foreignObject><p>x</p></foreignObject>')),
        contains('<foreignObject>'),
      );
      expect(
        iconProblem(_with('<image href="#a" width="9" height="9"/>')),
        contains('<image>'),
      );
      expect(iconProblem(_with('<filter id="f"/>')), contains('<filter>'));
      expect(iconProblem(_with('<g filter="url(#f)"/>')), contains('filters'));
      expect(iconProblem(_with('<style>*{}</style>')), contains('<style>'));
      expect(
        iconProblem(_with('<rect width="1" height="1" onclick="x()"/>')),
        contains('onclick'),
      );
    });

    test('refuses an external href or url()', () {
      expect(
        iconProblem(_with('<use xlink:href="https://evil.example/a.svg#x"/>')),
        contains('outside the file'),
      );
      expect(
        iconProblem(_with('<use href="other.svg#x"/>')),
        contains('outside the file'),
      );
      expect(
        iconProblem(_with('<rect width="1" height="1" fill="url(a.svg#g)"/>')),
        contains('outside the file'),
      );
      expect(iconProblem(_with('<use href="#x"/>')), isNull);
    });

    test('refuses a huge, malformed, non-svg or non-square file', () {
      final huge = _with('<path d="${'M0 0 L1 1 ' * 4000}"/>');
      expect(iconProblem(huge), contains('at most 32 KB'));
      expect(iconProblem('<svg viewBox="0 0 1 1">'), contains('well-formed'));
      expect(iconProblem('<g viewBox="0 0 10 10"/>'), contains('root element'));
      expect(
        iconProblem(_with('', box: '0 0 1024 512')),
        contains('not square'),
      );
      expect(
        iconProblem('<svg xmlns="http://www.w3.org/2000/svg"/>'),
        contains('square viewBox'),
      );
      expect(
        iconProblem('<!DOCTYPE svg [<!ENTITY a "b">]>${_with('')}'),
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
}
