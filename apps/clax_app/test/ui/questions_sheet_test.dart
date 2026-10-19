// The questions sheet: Clax's open questions about one app, read from the
// daemon and answered through it, one native card each.
import 'dart:async';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/questions/question_card.dart';
import 'package:clax_app/ui/questions/questions_sheet.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _units = {
  'question': 'Which unit should distances use?',
  'header': 'Units',
  'options': [
    {'label': 'Kilometres', 'description': 'As the rest of the app'},
    {'label': 'Miles'},
  ],
};

const _colour = {
  'question': 'Which colour should the header be?',
  'header': 'Colour',
  'options': [
    {'label': 'Green'},
    {'label': 'Orange'},
  ],
};

const _toppings = {
  'question': 'Which toppings go on the list?',
  'header': 'Toppings',
  'multi_select': true,
  'options': [
    {'label': 'Basil'},
    {'label': 'Olives'},
    {'label': 'Capers'},
  ],
};

class _Rig {
  _Rig(this.api, this.aid, this.sid);
  final FakeClaxApi api;
  final String aid;
  final String sid;
  final changes = StreamController<void>.broadcast();
  final nav = GlobalKey<NavigatorState>();
  Future<void>? shown;

  Future<String> ask(Map<String, Object?> q, {String? about}) async =>
      (await api.askQuestion(sid, [q], artifactId: about ?? aid)).question.id;

  List<FakeCall> callsOf(String name) => [
    for (final c in api.calls)
      if (c.name == name) c,
  ];
}

Future<_Rig> _rig() async {
  final api = FakeClaxApi();
  final aid = api.seedArtifact('Trips', const {'index.html': '<h1>Trips</h1>'});
  final s = await api.registerAppSession(installId: 'i', pid: 1);
  return _Rig(api, aid, s.id);
}

/// Reduced motion for the rest of the test: the questions sheet's mark
/// breathes until then, which no frame settles.
void _stillMotion(WidgetTester tester) {
  tester.platformDispatcher.accessibilityFeaturesTestValue =
      const FakeAccessibilityFeatures(disableAnimations: true);
  addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
}

/// A window of [size] on [platform], with the sheet's host page.
Future<void> _pump(
  WidgetTester tester,
  _Rig rig, {
  Size size = const Size(412, 915),
  TargetPlatform platform = TargetPlatform.android,
  double textScale = 1,
}) async {
  tester.view.physicalSize = size * 2;
  tester.view.devicePixelRatio = 2;
  addTearDown(tester.view.reset);
  _stillMotion(tester);
  addTearDown(rig.changes.close);
  await tester.pumpWidget(
    MaterialApp(
      theme: claxTheme(Brightness.light, platform),
      navigatorKey: rig.nav,
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(
          context,
        ).copyWith(textScaler: TextScaler.linear(textScale)),
        child: child!,
      ),
      home: const Scaffold(body: Center(child: Text('Trips'))),
    ),
  );
}

/// Opens the sheet over the page, focused on [focus].
Future<void> _open(WidgetTester tester, _Rig rig, {String? focus}) async {
  rig.shown = QuestionsSheet.show(
    rig.nav.currentContext!,
    api: rig.api,
    artifactId: rig.aid,
    focus: focus,
    changes: rig.changes.stream,
  );
  await tester.pumpAndSettle();
}

final _cards = find.byType(QuestionCard);

void main() {
  testWidgets('it shows a card for each open question about the app, one '
      'asked in a thread included', (tester) async {
    final rig = await _rig();
    final other = rig.api.seedArtifact('Notes', const {
      'index.html': '<h1>Notes</h1>',
    });
    await rig.ask(_units);
    final threaded = await rig.ask(_colour);
    rig.api.placeQuestionInThread(threaded, 'thread-1');
    await rig.ask(_toppings, about: other);
    await _pump(tester, rig);
    await _open(tester, rig);
    expect(find.text('Clax has 2 questions'), findsOneWidget);
    expect(_cards, findsNWidgets(2));
    expect(find.text('Which unit should distances use?'), findsOneWidget);
    expect(find.text('Which colour should the header be?'), findsOneWidget);
    expect(find.text('Which toppings go on the list?'), findsNothing);
    expect(find.text('Clax · In a thread'), findsOneWidget);
    expect(
      find.byWidgetPredicate(
        (w) => w is ClaxMark && w.state == ClaxMarkState.asking,
      ),
      findsOneWidget,
    );
  });

  testWidgets('a card says Clax, never the session name or its folder', (
    tester,
  ) async {
    final rig = await _rig();
    await rig.ask(_units);
    await _pump(tester, rig);
    await _open(tester, rig);
    expect(find.text('Clax has a question'), findsOneWidget);
    expect(find.text('Clax'), findsOneWidget);
    expect(find.textContaining('app-2'), findsNothing);
    expect(find.textContaining('in tmp'), findsNothing);
    expect(find.textContaining('a_app'), findsNothing);
  });

  testWidgets('a single choice and Answer send that label; the last answer '
      'closes the sheet', (tester) async {
    final rig = await _rig();
    final qid = await rig.ask(_units);
    await _pump(tester, rig);
    await _open(tester, rig);
    final answer = find.widgetWithText(ClaxButton, 'Answer');
    expect(
      tester.widget<ClaxButton>(answer).onPressed,
      isNull,
      reason: 'nothing is chosen yet',
    );
    expect(find.text('As the rest of the app'), findsOneWidget);
    await tester.tap(find.text('Miles'));
    await tester.pump();
    await tester.tap(answer);
    await tester.pumpAndSettle();
    expect(rig.callsOf('answerQuestion').single.args, {
      'qid': qid,
      'answers': [
        {
          'selected': ['Miles'],
          'text': null,
        },
      ],
    });
    expect(_cards, findsNothing);
    expect(find.byType(QuestionsSheetView), findsNothing);
    await rig.shown;
  });

  testWidgets('Skip declines the question', (tester) async {
    final rig = await _rig();
    final qid = await rig.ask(_units);
    await rig.ask(_colour);
    await _pump(tester, rig);
    await _open(tester, rig);
    await tester.tap(find.widgetWithText(ClaxButton, 'Skip').first);
    await tester.pumpAndSettle();
    expect(rig.callsOf('declineQuestion').single.args, {'qid': qid});
    expect(_cards, findsOneWidget);
    expect(find.text('Clax has a question'), findsOneWidget);
  });

  testWidgets('Other… sends what is typed', (tester) async {
    final rig = await _rig();
    final qid = await rig.ask(_units);
    await _pump(tester, rig);
    await _open(tester, rig);
    await tester.tap(find.text('Other…'));
    await tester.pump();
    await tester.enterText(find.byType(TextField), 'Nautical miles');
    await tester.pump();
    await tester.tap(find.widgetWithText(ClaxButton, 'Answer'));
    await tester.pumpAndSettle();
    expect(rig.callsOf('answerQuestion').single.args['answers'], [
      {'selected': <String>[], 'text': 'Nautical miles'},
    ]);
    expect(rig.callsOf('answerQuestion').single.args['qid'], qid);
  });

  testWidgets('a multiple choice sends every label picked', (tester) async {
    final rig = await _rig();
    await rig.ask(_toppings);
    await _pump(tester, rig);
    await _open(tester, rig);
    await tester.tap(find.text('Basil'));
    await tester.tap(find.text('Capers'));
    await tester.pump();
    await tester.tap(find.widgetWithText(ClaxButton, 'Answer'));
    await tester.pumpAndSettle();
    expect(rig.callsOf('answerQuestion').single.args['answers'], [
      {
        'selected': ['Basil', 'Capers'],
        'text': null,
      },
    ]);
  });

  testWidgets('a question answered elsewhere drops its card on Answer '
      '(409)', (tester) async {
    final rig = await _rig();
    final qid = await rig.ask(_units);
    await rig.ask(_colour);
    await _pump(tester, rig);
    await _open(tester, rig);
    await rig.api.declineQuestion(qid);
    await tester.tap(find.text('Miles'));
    await tester.pump();
    await tester.tap(find.widgetWithText(ClaxButton, 'Answer').first);
    await tester.pumpAndSettle();
    expect(_cards, findsOneWidget);
    expect(find.text('Which unit should distances use?'), findsNothing);
    expect(find.text('Which colour should the header be?'), findsOneWidget);
  });

  testWidgets('a change reads the questions again: a new one is added and '
      'a closed one goes, keeping what was chosen', (tester) async {
    final rig = await _rig();
    final first = await rig.ask(_units);
    final second = await rig.ask(_colour);
    await _pump(tester, rig);
    await _open(tester, rig);
    await tester.tap(find.text('Green'));
    await tester.pump();
    await rig.api.declineQuestion(first);
    await rig.ask(_toppings);
    rig.changes.add(null);
    await tester.pumpAndSettle();
    expect(_cards, findsNWidgets(2));
    expect(find.text('Which unit should distances use?'), findsNothing);
    expect(find.text('Which toppings go on the list?'), findsOneWidget);
    await tester.tap(find.widgetWithText(ClaxButton, 'Answer').first);
    await tester.pumpAndSettle();
    expect(rig.callsOf('answerQuestion').single.args, {
      'qid': second,
      'answers': [
        {
          'selected': ['Green'],
          'text': null,
        },
      ],
    });
  });

  testWidgets('the focused question comes first', (tester) async {
    final rig = await _rig();
    await rig.ask(_units);
    final second = await rig.ask(_colour);
    await _pump(tester, rig);
    await _open(tester, rig, focus: second);
    final cards = tester.widgetList<QuestionCard>(_cards).toList();
    expect(cards.first.question.id, second);
  });

  testWidgets('with nothing open it closes at once', (tester) async {
    final rig = await _rig();
    await _pump(tester, rig);
    await _open(tester, rig);
    expect(find.byType(QuestionsSheetView), findsNothing);
    await rig.shown;
  });

  testWidgets('compact: a bottom sheet with a handle', (tester) async {
    final rig = await _rig();
    await rig.ask(_units);
    await _pump(tester, rig);
    await _open(tester, rig);
    expect(find.byType(BottomSheet), findsOneWidget);
    expect(find.byType(Dialog), findsNothing);
  });

  testWidgets('medium and up: a centred dialog', (tester) async {
    final rig = await _rig();
    await rig.ask(_units);
    await _pump(tester, rig, size: const Size(1280, 800));
    await _open(tester, rig);
    expect(find.byType(Dialog), findsOneWidget);
    expect(find.byType(BottomSheet), findsNothing);
  });

  testWidgets('the Mac: a 560 sheet from the top of the window', (
    tester,
  ) async {
    final rig = await _rig();
    await rig.ask(_units);
    await _pump(
      tester,
      rig,
      size: const Size(1200, 800),
      platform: TargetPlatform.macOS,
    );
    await _open(tester, rig);
    final panel = tester.getRect(find.byKey(ClaxSheet.appleSheetKey));
    expect(panel.width, 560);
  });

  testWidgets('at 200% text Answer is reached by scrolling', (tester) async {
    final rig = await _rig();
    await rig.ask(const {
      'question':
          'Which of these layouts should the trip page use on a phone held '
          'upright?',
      'header': 'Layout',
      'options': [
        {'label': 'One column', 'description': 'Every leg under the last'},
        {'label': 'Two columns', 'description': 'Legs beside their maps'},
        {'label': 'Cards', 'description': 'One leg a card, swiped across'},
        {'label': 'Timeline', 'description': 'Legs on a line down the page'},
      ],
    });
    await _pump(tester, rig, textScale: 2);
    await _open(tester, rig);
    await tester.ensureVisible(find.text('Timeline'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Timeline'));
    await tester.pump();
    final answer = find.widgetWithText(ClaxButton, 'Answer');
    await tester.ensureVisible(answer);
    await tester.pumpAndSettle();
    await tester.tap(answer);
    await tester.pumpAndSettle();
    expect(rig.callsOf('answerQuestion'), hasLength(1));
  });
}
