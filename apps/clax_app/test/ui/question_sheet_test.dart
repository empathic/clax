import 'dart:async';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/ui/question_sheet.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A [QuestionPage] that records what it loads.
class FakeQuestionPage implements QuestionPage {
  final loads = <Uri>[];
  var disposed = false;

  @override
  Widget build(BuildContext context) =>
      const ColoredBox(key: ValueKey('fake-question-page'), color: Colors.teal);

  @override
  Future<void> load(Uri url) async => loads.add(url);

  @override
  void dispose() => disposed = true;
}

QuestionView view(String id, String status) => QuestionView.fromJson({
  'id': id,
  'source': 'ask',
  'status': status,
  'questions': [
    {
      'question': 'Which unit?',
      'header': 'Units',
      'options': [
        {'label': 'km', 'description': ''},
        {'label': 'mi', 'description': ''},
      ],
      'multi_select': false,
      'other': true,
    },
  ],
  'created_at': '2026-10-13T00:00:00Z',
});

void main() {
  final base = Uri.parse('http://127.0.0.1:7999');

  test("the sheet's page is the shell's inbox at the question", () {
    expect(
      questionUrl(base, 'q 1').toString(),
      'http://127.0.0.1:7999/inbox?q=q+1',
    );
  });

  test('the page may stay on the inbox of its daemon, nowhere else', () {
    bool ok(String u) => questionPageAllows(base, u);
    expect(ok('http://127.0.0.1:7999/inbox?q=abc'), isTrue);
    expect(ok('http://127.0.0.1:7999/inbox'), isTrue);
    expect(ok('http://127.0.0.1:7999/a/abcdefghjkmn'), isFalse);
    expect(ok('http://127.0.0.1:7998/inbox?q=abc'), isFalse);
    expect(ok('https://example.com/inbox'), isFalse);
    expect(ok('http://user@127.0.0.1:7999/inbox'), isFalse);
  });

  Future<
    ({
      FakeQuestionPage page,
      StreamController<QuestionView> updates,
      Future<void> shown,
    })
  >
  pumpSheet(WidgetTester tester, {Size size = const Size(400, 800)}) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final page = FakeQuestionPage();
    final updates = StreamController<QuestionView>.broadcast();
    addTearDown(updates.close);
    late Future<void> shown;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => shown = showQuestionSheet(
              context,
              page: page,
              base: base,
              questionId: 'q1',
              updates: updates.stream,
            ),
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    return (page: page, updates: updates, shown: shown);
  }

  testWidgets('it loads the question and closes once it is answered', (
    tester,
  ) async {
    final s = await pumpSheet(tester);
    expect(s.page.loads, [questionUrl(base, 'q1')]);
    expect(find.byKey(const ValueKey('fake-question-page')), findsOneWidget);
    expect(find.text('Clax has a question'), findsOneWidget);

    s.updates.add(view('q2', 'answered'));
    s.updates.add(view('q1', 'open'));
    await tester.pumpAndSettle();
    expect(
      find.byKey(const ValueKey('fake-question-page')),
      findsOneWidget,
      reason: "another question's close, or this one still open, keeps it",
    );

    s.updates.add(view('q1', 'answered'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('fake-question-page')), findsNothing);
    expect(s.page.disposed, isTrue);
    await s.shown;
  });

  for (final status in ['declined', 'withdrawn']) {
    testWidgets('it closes once the question is $status', (tester) async {
      final s = await pumpSheet(tester);
      s.updates.add(view('q1', status));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('fake-question-page')), findsNothing);
    });
  }

  testWidgets('Close closes it', (tester) async {
    final s = await pumpSheet(tester);
    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('fake-question-page')), findsNothing);
    expect(s.page.disposed, isTrue);
  });

  testWidgets('on a wide screen it is a dialog', (tester) async {
    await pumpSheet(tester, size: const Size(1200, 800));
    expect(find.byType(Dialog), findsOneWidget);
    expect(find.byKey(const ValueKey('fake-question-page')), findsOneWidget);
  });
}
