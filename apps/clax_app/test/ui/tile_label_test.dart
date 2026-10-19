import 'package:clax_app/ui/tile_label.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

// The test font draws every character 1 em wide: at 10 px, 14 characters
// fit in 140.
const _style = TextStyle(fontSize: 10, height: 1.2);
const _long = 'Donaudampfschifffahrtsgesellschaftskapitän';

TileLabelLayout _at(String text, {double scale = 1, int maxLines = 2}) =>
    tileLabelLayout(
      text,
      _style,
      140,
      TextScaler.linear(scale),
      maxLines: maxLines,
    );

void main() {
  group('tileLabelLayout', () {
    test('a title that fits is one line', () {
      expect(
        _at('Weekly budget'),
        const TileLabelLayout(['Weekly budget'], ellipsis: false),
      );
    });

    test('a longer title breaks between words, the rest cut on line 2', () {
      expect(
        _at('A tide table for the harbour'),
        const TileLabelLayout([
          'A tide table',
          'for the harbour',
        ], ellipsis: true),
      );
    });

    test('a word wider than the cell is that word alone, ellipsized', () {
      expect(_long.length, 42);
      expect(_at(_long), const TileLabelLayout([_long], ellipsis: true));
      expect(_at('$_long log'), const TileLabelLayout([_long], ellipsis: true));
    });

    test('nothing breaks mid-word, at 100% and 200%', () {
      for (final scale in [1.0, 2.0]) {
        for (final text in [
          'Kindergarten pickup rota',
          'A tide table for the harbour',
          'Weekly budget',
          _long,
        ]) {
          final l = _at(text, scale: scale);
          expect(l.lines.length, lessThanOrEqualTo(2), reason: '$text @$scale');
          final words = text.split(' ').toSet();
          for (final line in l.lines) {
            for (final w in line.split(' ')) {
              expect(words, contains(w), reason: '$text @$scale');
            }
          }
        }
      }
    });

    test('at 200% a title takes two lines at most, breaking earlier', () {
      expect(
        _at('A tide table for the harbour', scale: 2),
        const TileLabelLayout([
          'A tide',
          'table for the harbour',
        ], ellipsis: true),
      );
    });

    test('with maxLines 1 the whole title is one line', () {
      expect(
        _at('A packing list for trips', maxLines: 1),
        const TileLabelLayout(['A packing list for trips'], ellipsis: true),
      );
    });
  });

  group('TileLabel', () {
    Future<List<RenderParagraph>> pump(
      WidgetTester tester,
      String text, {
      double scale = 1,
    }) async {
      await tester.pumpWidget(
        MaterialApp(
          home: MediaQuery(
            data: MediaQueryData(textScaler: TextScaler.linear(scale)),
            child: Center(
              child: SizedBox(width: 160, child: TileLabel(text)),
            ),
          ),
        ),
      );
      return tester
          .renderObjectList<RenderParagraph>(
            find.descendant(
              of: find.byType(TileLabel),
              matching: find.byType(RichText),
            ),
          )
          .toList();
    }

    String shown(RenderParagraph p) => p.text.toPlainText();

    testWidgets('draws whole words on each line, two at most', (tester) async {
      // The caption is 12 px: 13 characters in 160.
      final lines = await pump(tester, 'Kindergarten pickup rota');
      expect(lines.map(shown), ['Kindergarten', 'pickup rota']);
      expect(lines.any((l) => l.didExceedMaxLines), isFalse);
    });

    testWidgets('a short title is one line', (tester) async {
      final lines = await pump(tester, 'Notes');
      expect(lines.map(shown), ['Notes']);
      expect(lines.single.didExceedMaxLines, isFalse);
    });

    testWidgets('a 42-character word is one ellipsized line', (tester) async {
      final lines = await pump(tester, _long);
      expect(lines.map(shown), [_long]);
      expect(lines.single.didExceedMaxLines, isTrue);
    });

    testWidgets('at 200% still two lines at most', (tester) async {
      final lines = await pump(
        tester,
        'A tide table for the harbour',
        scale: 2,
      );
      expect(lines, hasLength(2));
      expect(lines.map(shown), ['A tide', 'table for the harbour']);
      expect(tester.takeException(), isNull);
    });
  });
}
