import 'dart:math';

import 'package:clax_app/ui/haiku.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

Widget _app(Widget child, {TargetPlatform platform = TargetPlatform.android}) =>
    MaterialApp(
      theme: claxTheme(Brightness.light, platform),
      home: Scaffold(body: Center(child: child)),
    );

void main() {
  group('pickHaiku', () {
    // The shell's pickHaiku (web/shell/src/view/haiku.ts) hashes the seed's
    // UTF-16 code units with 32-bit FNV-1a: 'x' is 4245442695, 'clax'
    // 1260718619, 'r1' 206831020 and '' 2166136261.
    test('a seed picks as the shell does', () {
      const three = ['a', 'b', 'c'];
      expect(pickHaiku(three, seed: 'x'), 'a');
      expect(pickHaiku(three, seed: 'clax'), 'c');
      expect(pickHaiku(three, seed: 'r1'), 'b');
      expect(pickHaiku(three, seed: ''), 'b');
      const seven = ['0', '1', '2', '3', '4', '5', '6'];
      expect(pickHaiku(seven, seed: 'x'), '4');
      expect(pickHaiku(seven, seed: 'clax'), '6');
    });

    test('a seed picks the same one every time', () {
      final list = [for (var i = 0; i < 10; i++) 'h$i'];
      final first = pickHaiku(list, seed: 'app-42');
      for (var i = 0; i < 5; i++) {
        expect(pickHaiku(list, seed: 'app-42'), first);
      }
    });

    test('without a seed it picks with the random given', () {
      final list = [for (var i = 0; i < 10; i++) 'h$i'];
      final a = Random(7);
      final b = Random(7);
      for (var i = 0; i < 5; i++) {
        expect(pickHaiku(list, random: a), list[b.nextInt(list.length)]);
      }
    });
  });

  test('loadHaikus reads the shared list, and a missing one is empty', () async {
    final list = await loadHaikus(_Bundle('["one\\ntwo\\nthree", "four"]'));
    expect(list, ['one\ntwo\nthree', 'four']);
    expect(await loadHaikus(_Bundle(null)), isEmpty);
    expect(await loadHaikus(_Bundle('{"not": "a list"}')), isEmpty);
  });

  group('HaikuLine', () {
    const text = 'Localhost at dusk\nseventy-four eighty hums\nno cloud overhead';

    testWidgets('is read as a haiku when focused, never as a live region', (
      tester,
    ) async {
      final handle = tester.ensureSemantics();
      await tester.pumpWidget(_app(const HaikuLine(text: text)));
      final node = tester.getSemantics(find.byType(HaikuLine));
      expect(node.label, 'Haiku: $text');
      expect(node.getSemanticsData().flagsCollection.isLiveRegion, isFalse);
      handle.dispose();
    });

    testWidgets('draws in the haiku style, muted', (tester) async {
      await tester.pumpWidget(_app(const HaikuLine(text: text)));
      final t = tester.widget<Text>(find.text(text));
      final p = claxPalette(tester.element(find.text(text)));
      expect(t.style?.fontStyle, FontStyle.italic);
      expect(t.style?.color, p.muted);
    });

    testWidgets('a long-press asks for another', (tester) async {
      var another = 0;
      await tester.pumpWidget(
        _app(HaikuLine(text: text, onAnother: () => another++)),
      );
      await tester.tap(find.text(text));
      expect(another, 0);
      await tester.longPress(find.text(text));
      expect(another, 1);
    });

    testWidgets('on the Mac an Option-click asks for another; a click does '
        'not', (tester) async {
      var another = 0;
      await tester.pumpWidget(
        _app(
          HaikuLine(text: text, onAnother: () => another++),
          platform: TargetPlatform.macOS,
        ),
      );
      await tester.tap(find.text(text));
      expect(another, 0);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
      await tester.tap(find.text(text));
      await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
      expect(another, 1);
    });
  });
}

class _Bundle extends CachingAssetBundle {
  _Bundle(this.text);
  final String? text;

  @override
  Future<ByteData> load(String key) async {
    final t = text;
    if (t == null || key != haikuAsset) throw FlutterError('no $key');
    return ByteData.sublistView(Uint8List.fromList(t.codeUnits));
  }
}
