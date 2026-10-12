import 'package:clax_app/ui/comment_button.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  /// The button in a 360 x 700 area.
  Future<List<String>> pumpButton(
    WidgetTester tester, {
    double dy = CommentButton.defaultDy,
    bool commenting = false,
    int threads = 0,
    List<double>? moves,
  }) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(360, 700);
    addTearDown(tester.view.reset);
    final log = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light),
        home: Scaffold(
          body: CommentButton(
            commenting: commenting,
            threadCount: threads,
            onTap: () => log.add('tap'),
            onLongPress: () => log.add('long'),
            dy: dy,
            onMoved: (d) => moves?.add(d),
          ),
        ),
      ),
    );
    return log;
  }

  Rect button(WidgetTester tester) =>
      tester.getRect(find.byKey(const ValueKey('comment-button')));

  testWidgets('sits bottom right, dy above the bottom', (tester) async {
    await pumpButton(tester);
    final r = button(tester);
    expect(r.size, const Size(56, 56));
    expect(r.right, 360 - CommentButton.margin);
    expect(r.bottom, 700 - CommentButton.defaultDy);
  });

  testWidgets('a dy out of range is kept on screen', (tester) async {
    await pumpButton(tester, dy: 5000);
    expect(button(tester).top, CommentButton.topReserve);
    await pumpButton(tester, dy: -40);
    await tester.pumpAndSettle();
    expect(button(tester).bottom, 700 - CommentButton.margin);
  });

  testWidgets('tap and long-press', (tester) async {
    final log = await pumpButton(tester);
    await tester.tap(find.byKey(const ValueKey('comment-button')));
    await tester.longPress(find.byKey(const ValueKey('comment-button')));
    expect(log, ['tap', 'long']);
  });

  testWidgets('a drag snaps to the nearer edge and reports its height', (
    tester,
  ) async {
    final moves = <double>[];
    final log = await pumpButton(tester, moves: moves);
    final from = button(tester).center;
    // Up 200 and most of the way left: it lands on the left edge.
    await tester.dragFrom(from, const Offset(-220, -200));
    await tester.pumpAndSettle();
    expect(log, isEmpty, reason: 'a drag is not a tap');
    expect(moves, hasLength(1));
    expect(moves.single, closeTo(CommentButton.defaultDy + 200, 1));
    final r = button(tester);
    expect(r.left, CommentButton.margin);

    // A little to the right stays on the left.
    await tester.dragFrom(r.center, const Offset(60, 0));
    await tester.pumpAndSettle();
    expect(button(tester).left, CommentButton.margin);
    // Past the middle goes right.
    await tester.dragFrom(button(tester).center, const Offset(200, 0));
    await tester.pumpAndSettle();
    expect(button(tester).right, 360 - CommentButton.margin);
  });

  testWidgets('a drag past the top or bottom stops at the bounds', (
    tester,
  ) async {
    final moves = <double>[];
    await pumpButton(tester, moves: moves);
    await tester.dragFrom(button(tester).center, const Offset(0, -2000));
    await tester.pumpAndSettle();
    expect(moves.last, 700 - 56 - CommentButton.topReserve);
    await tester.dragFrom(button(tester).center, const Offset(0, 2000));
    await tester.pumpAndSettle();
    expect(moves.last, CommentButton.margin);
  });

  testWidgets('open threads show as a badge, not while commenting', (
    tester,
  ) async {
    await pumpButton(tester, threads: 3);
    expect(find.text('3'), findsOneWidget);
    expect(find.byIcon(Icons.add_comment_outlined), findsOneWidget);
    await pumpButton(tester, threads: 3, commenting: true);
    expect(find.text('3'), findsNothing);
    expect(find.byIcon(Icons.close), findsOneWidget);
  });

  testWidgets('outside the button, touches reach what is under it', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(360, 700);
    addTearDown(tester.view.reset);
    var under = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Stack(
          children: [
            GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: () => under++,
              child: const SizedBox.expand(),
            ),
            CommentButton(
              commenting: false,
              threadCount: 0,
              onTap: () {},
              onLongPress: () {},
              dy: CommentButton.defaultDy,
              onMoved: (_) {},
            ),
          ],
        ),
      ),
    );
    await tester.tapAt(const Offset(100, 300));
    expect(under, 1);
  });

  testWidgets('the comment bar calls back and counts threads', (tester) async {
    final log = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          bottomNavigationBar: CommentBar(
            threadCount: 2,
            onWholeApp: () => log.add('whole'),
            onThreads: () => log.add('threads'),
            onClose: () => log.add('close'),
          ),
        ),
      ),
    );
    expect(find.text('Hold on what you want changed'), findsOneWidget);
    expect(find.text('Threads (2)'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('comment-whole-app')));
    await tester.tap(find.byKey(const ValueKey('comment-threads')));
    await tester.tap(find.byKey(const ValueKey('comment-bar-close')));
    expect(log, ['whole', 'threads', 'close']);
  });
}
