import 'package:clax_app/ui/kit/window_titlebar.dart';
import 'package:clax_app/ui/mac/window_drag.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A 52-point row with the window buttons' corner 74 points wide.
const _bar = WindowTitlebar(top: 52, leading: 74);

void main() {
  late int drags;
  late int backs;

  Future<void> pump(WidgetTester tester) async {
    drags = 0;
    backs = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: WindowRowDrag(
          titlebar: _bar,
          onDrag: () => drags++,
          child: Scaffold(
            body: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  height: 52,
                  child: Row(
                    children: [
                      const SizedBox(width: 78),
                      IconButton(
                        key: const ValueKey('back'),
                        onPressed: () => backs++,
                        icon: const Icon(Icons.arrow_back),
                      ),
                      const Text('Inbox'),
                    ],
                  ),
                ),
                TextButton(onPressed: () {}, child: const Text('below')),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Future<TestGesture> press(WidgetTester tester, Offset at) async {
    final g = await tester.startGesture(at, kind: PointerDeviceKind.mouse);
    await tester.pump();
    return g;
  }

  testWidgets('a press on no control in the row moves the window at once', (
    tester,
  ) async {
    await pump(tester);
    final g = await press(tester, const Offset(600, 26));
    expect(drags, 1);
    await g.up();
    // The title is no control either.
    final t = await press(tester, tester.getCenter(find.text('Inbox')));
    await t.up();
    expect(drags, 2);
  });

  testWidgets('a click on a control in the row is the control\'s', (
    tester,
  ) async {
    await pump(tester);
    final at = tester.getCenter(find.byKey(const ValueKey('back')));
    final g = await press(tester, at);
    // A wobble inside the button's slop is still a click.
    await g.moveBy(const Offset(4, 0));
    await g.up();
    await tester.pump();
    expect(backs, 1);
    expect(drags, 0);
  });

  testWidgets('a press dragged off a control moves the window', (tester) async {
    await pump(tester);
    final at = tester.getCenter(find.byKey(const ValueKey('back')));
    final g = await press(tester, at);
    expect(drags, 0);
    await g.moveBy(const Offset(40, 0));
    await tester.pump();
    expect(drags, 1);
    await g.up();
    await tester.pump();
    expect(backs, 0);
  });

  testWidgets('presses below the row, in the window buttons\' corner, with '
      'another button or by touch move nothing', (tester) async {
    await pump(tester);
    for (final at in [const Offset(600, 300), const Offset(40, 26)]) {
      final g = await press(tester, at);
      await g.up();
    }
    final right = await tester.startGesture(
      const Offset(600, 26),
      kind: PointerDeviceKind.mouse,
      buttons: kSecondaryMouseButton,
    );
    await tester.pump();
    await right.up();
    final touch = await tester.startGesture(const Offset(600, 26));
    await tester.pump();
    await touch.up();
    expect(drags, 0);
  });
}
