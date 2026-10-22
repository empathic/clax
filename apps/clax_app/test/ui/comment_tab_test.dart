import 'package:clax_app/ui/comment_tab.dart';
import 'package:clax_app/ui/kit/mark.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

final _tab = find.byKey(const ValueKey('comment-tab'));
final _label = find.descendant(of: _tab, matching: find.text('Comment'));

void main() {
  late List<String> log;
  late List<double> moved;

  setUp(() {
    log = [];
    moved = [];
  });

  /// A 400×800 phone with [gestureRight] dp of back-gesture zone on the
  /// right.
  Future<void> pump(
    WidgetTester tester, {
    double at = 0.62,
    int threads = 2,
    bool change = false,
    double textScale = 1,
    double gestureRight = 32,
  }) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(400, 800);
    tester.view.systemGestureInsets = FakeViewPadding(right: gestureRight);
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, TargetPlatform.android),
        home: MediaQuery.withClampedTextScaling(
          minScaleFactor: textScale,
          maxScaleFactor: textScale,
          child: Scaffold(
            body: Stack(
              children: [
                const Positioned.fill(child: ColoredBox(color: Colors.white)),
                Positioned.fill(
                  child: CommentTab(
                    threads: threads,
                    onComment: () => log.add('comment'),
                    onThreads: () => log.add('threads'),
                    change: change
                        ? (
                            label: 'Change in progress',
                            onOpen: () => log.add('change'),
                          )
                        : null,
                    at: at,
                    onMoved: moved.add,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets('it sits 4 dp inside the back-gesture zone, at 62% of the '
      'height, labelled Comment with the mark', (tester) async {
    await pump(tester);
    final box = tester.getRect(_tab);
    expect(box.right, 400 - 36);
    expect(box.center.dy, closeTo(800 * 0.62, 0.5));
    expect(box.height, 48, reason: 'a 48 dp target');
    expect(_label, findsOneWidget);
    expect(
      find.descendant(of: _tab, matching: find.byType(ClaxMark)),
      findsOneWidget,
    );
    final mark = tester.widget<ClaxMark>(
      find.descendant(of: _tab, matching: find.byType(ClaxMark)),
    );
    expect(mark.size, 16);
    final face = tester.getRect(find.byKey(const ValueKey('comment-tab-face')));
    expect(face.height, 44);
  });

  testWidgets('with no gesture zone it is still 4 dp from the edge', (
    tester,
  ) async {
    await pump(tester, gestureRight: 0);
    expect(tester.getRect(_tab).right, 400 - 4);
  });

  testWidgets('it is filled with fg and its words are bg', (tester) async {
    await pump(tester);
    final context = tester.element(_tab);
    final p = claxPalette(context);
    final face = tester.widget<DecoratedBox>(
      find.byKey(const ValueKey('comment-tab-face')),
    );
    expect((face.decoration as BoxDecoration).color, p.fg);
    expect(tester.widget<Text>(_label).style?.color, p.bg);
  });

  testWidgets('a horizontal drag does nothing', (tester) async {
    await pump(tester);
    final before = tester.getRect(_tab);
    await tester.drag(_tab, const Offset(-120, 0));
    await tester.pumpAndSettle();
    expect(tester.getRect(_tab), before);
    expect(moved, isEmpty);
    expect(log, isEmpty);
    expect(find.text('Threads (2)'), findsNothing, reason: 'no menu');
  });

  testWidgets('a plain vertical drag does nothing either', (tester) async {
    await pump(tester);
    final before = tester.getRect(_tab);
    await tester.drag(_tab, const Offset(0, -120));
    await tester.pumpAndSettle();
    expect(tester.getRect(_tab), before);
    expect(moved, isEmpty);
  });

  testWidgets('a long-press then a vertical drag moves it and reports the new '
      'fraction', (tester) async {
    await pump(tester);
    final start = tester.getCenter(_tab);
    final g = await tester.startGesture(start);
    await tester.pump(kLongPressTimeout + const Duration(milliseconds: 50));
    await g.moveBy(const Offset(0, -50));
    await tester.pump();
    await g.moveBy(const Offset(0, -50));
    await tester.pump();
    expect(tester.getCenter(_tab).dy, closeTo(start.dy - 100, 0.5));
    expect(tester.getRect(_tab).right, 400 - 36, reason: 'only vertical');
    await g.up();
    await tester.pumpAndSettle();
    expect(moved, hasLength(1));
    expect(moved.single, closeTo((start.dy - 100) / 800, 0.001));
    expect(log, isEmpty, reason: 'a move is not a tap');
    expect(find.text('Threads (2)'), findsNothing);
  });

  testWidgets('a move stops at the screen edges', (tester) async {
    await pump(tester);
    final g = await tester.startGesture(tester.getCenter(_tab));
    await tester.pump(kLongPressTimeout + const Duration(milliseconds: 50));
    await g.moveBy(const Offset(0, 2000));
    await tester.pump();
    await g.up();
    await tester.pumpAndSettle();
    expect(tester.getRect(_tab).bottom, lessThanOrEqualTo(800));
    expect(moved.single, lessThan(1));
  });

  testWidgets('a tap opens Comment and Threads (n)', (tester) async {
    await pump(tester, threads: 3);
    await tester.tap(_tab);
    await tester.pumpAndSettle();
    expect(find.text('Threads (3)'), findsOneWidget);
    expect(find.text('Change in progress'), findsNothing);
    await tester.tap(find.byKey(const ValueKey('comment')));
    await tester.pumpAndSettle();
    expect(log, ['comment']);

    await tester.tap(_tab);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Threads (3)'));
    await tester.pumpAndSettle();
    expect(log, ['comment', 'threads']);
  });

  testWidgets('with a change running the menu has three items, the change '
      'last', (tester) async {
    await pump(tester, change: true);
    await tester.tap(_tab);
    await tester.pumpAndSettle();
    final items = [
      find.byKey(const ValueKey('comment')),
      find.byKey(const ValueKey('threads')),
      find.byKey(const ValueKey('change')),
    ];
    for (final i in items) {
      expect(i, findsOneWidget);
    }
    expect(
      tester.getTopLeft(items[2]).dy,
      greaterThan(tester.getTopLeft(items[1]).dy),
    );
    await tester.tap(find.text('Change in progress'));
    await tester.pumpAndSettle();
    expect(log, ['change']);
  });

  testWidgets('at 200% the label is not clipped and it stays inside the '
      'screen', (tester) async {
    await pump(tester, textScale: 2);
    final label = tester.getRect(_label);
    final face = tester.getRect(find.byKey(const ValueKey('comment-tab-face')));
    expect(face.contains(label.topLeft), isTrue);
    expect(face.contains(label.bottomRight - const Offset(0.01, 0.01)), isTrue);
    final painter = tester.renderObject<RenderParagraph>(_label);
    expect(painter.didExceedMaxLines, isFalse);
    expect(tester.getRect(_tab).left, greaterThanOrEqualTo(0));
    expect(tester.takeException(), isNull);
  });

  testWidgets('it reads as the Comment button', (tester) async {
    final handle = tester.ensureSemantics();
    await pump(tester);
    expect(
      tester.getSemantics(_tab),
      matchesSemantics(
        label: 'Comment',
        isButton: true,
        hasTapAction: true,
      ),
    );
    handle.dispose();
  });
}
