import 'package:clax_app/ui/error_screen.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/log_details.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

const _reason =
    'clax stopped 5 times within a minute; last: exited with code 1';
const _tail = ['error: database is locked', 'clax exiting'];

void main() {
  late int restarts;
  late List<String> copied;

  setUp(() {
    restarts = 0;
    copied = [];
  });

  Future<void> pumpError(
    WidgetTester tester, {
    TargetPlatform platform = TargetPlatform.android,
    List<String> tail = _tail,
  }) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied.add((call.arguments as Map)['text'] as String);
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, platform),
        home: ErrorScreen(
          reason: _reason,
          tail: tail,
          onRestart: () => restarts++,
        ),
      ),
    );
  }

  testWidgets('says Clax couldn’t start plainly, centred, as one ClaxEmpty', (
    tester,
  ) async {
    await pumpError(tester);
    expect(find.byType(ClaxEmpty), findsOneWidget);
    const title = 'Clax couldn’t start';
    const body =
        'It stopped while starting. Try again, or copy the details for a '
        'report.';
    for (final t in [title, body]) {
      expect(tester.widget<Text>(find.text(t)).textAlign, TextAlign.center);
    }
    expect(find.textContaining('clax stopped'), findsNothing);
    expect(find.textContaining('within a minute'), findsNothing);
    expect(find.textContaining('The last lines'), findsNothing);
    expect(find.text('Restart'), findsNothing);
  });

  testWidgets('Try again starts Clax again', (tester) async {
    await pumpError(tester);
    await tester.tap(find.text('Try again'));
    await tester.pump();
    expect(restarts, 1);
  });

  testWidgets('the log is closed under Details, and opens in mono, '
      'scrolling sideways', (tester) async {
    await pumpError(tester);
    expect(find.textContaining('database is locked'), findsNothing);
    await tester.tap(find.text('Details'));
    await tester.pump();
    final log = find.text('error: database is locked\nclax exiting');
    expect(log, findsOneWidget);
    expect(
      tester.widget<Text>(log).style?.fontFamily,
      ClaxType.mono(TargetPlatform.android).fontFamily,
    );
    final sideways = find.ancestor(
      of: log,
      matching: find.byWidgetPredicate(
        (w) => w is SingleChildScrollView && w.scrollDirection == Axis.horizontal,
      ),
    );
    expect(sideways, findsOneWidget);
    await tester.tap(find.text('Details'));
    await tester.pump();
    expect(log, findsNothing);
  });

  testWidgets('a long line fades at the edge until it is scrolled to its end', (
    tester,
  ) async {
    await pumpError(tester, tail: ['x' * 400]);
    await tester.tap(find.text('Details'));
    await tester.pump();
    await tester.pump();
    expect(find.byKey(LogDetails.fadeKey), findsOneWidget);
    final scroll = find.byWidgetPredicate(
      (w) => w is SingleChildScrollView && w.scrollDirection == Axis.horizontal,
    );
    final position = tester
        .state<ScrollableState>(
          find.descendant(of: scroll, matching: find.byType(Scrollable)),
        )
        .position;
    expect(position.maxScrollExtent, greaterThan(0));
    position.jumpTo(position.maxScrollExtent);
    await tester.pump();
    expect(find.byKey(LogDetails.fadeKey), findsNothing);
  });

  testWidgets('a drag scrolls a long line sideways', (tester) async {
    await pumpError(tester, tail: ['x' * 400]);
    await tester.tap(find.text('Details'));
    await tester.pump();
    final scroll = find.byWidgetPredicate(
      (w) => w is SingleChildScrollView && w.scrollDirection == Axis.horizontal,
    );
    await tester.drag(scroll, const Offset(-200, 0));
    await tester.pumpAndSettle();
    final position = tester
        .state<ScrollableState>(
          find.descendant(of: scroll, matching: find.byType(Scrollable)),
        )
        .position;
    expect(position.pixels, greaterThan(100));
  });

  testWidgets('a short log has no fade', (tester) async {
    await pumpError(tester, tail: ['ok']);
    await tester.tap(find.text('Details'));
    await tester.pump();
    await tester.pump();
    expect(find.byKey(LogDetails.fadeKey), findsNothing);
  });

  testWidgets('Copy details puts the reason and the log on the clipboard, '
      'and says so', (tester) async {
    await pumpError(tester);
    await tester.tap(find.text('Copy details'));
    await tester.pump();
    expect(copied, ['$_reason\n\n${_tail.join('\n')}']);
    expect(find.text('Copied'), findsOneWidget);
  });

  testWidgets('on the Mac it is the same state in the Apple family', (
    tester,
  ) async {
    await pumpError(tester, platform: TargetPlatform.macOS);
    expect(find.text('Clax couldn’t start'), findsOneWidget);
    expect(find.text('Try again'), findsOneWidget);
    expect(find.text('Copy details'), findsOneWidget);
  });
}
