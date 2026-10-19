// The questions sheet rendered into CLAX_SHOTS_DIR (skipped without it):
// three of Clax's questions about one app, one of them asked in a thread,
// over the app's page, on the phone (also at 130% and 200% text), the
// iPhone, a wide window and the Mac.
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/questions/questions_sheet.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

Future<void> _sheet(Shot s) async {
  final api = FakeClaxApi();
  final aid = api.seedArtifact('Trips', const {'index.html': '<h1>Trips</h1>'});
  final session = await api.registerAppSession(installId: 'i', pid: 1);
  Future<String> ask(Map<String, Object?> q) async =>
      (await api.askQuestion(session.id, [q], artifactId: aid)).question.id;
  await ask(const {
    'question': 'Which unit should distances use?',
    'header': 'Units',
    'options': [
      {
        'label': 'Kilometres',
        'description': 'As the rest of the app',
        'recommended': true,
      },
      {'label': 'Miles', 'description': 'As your phone’s region'},
    ],
  });
  final threaded = await ask(const {
    'question': 'Which details should each leg show?',
    'header': 'Legs',
    'multi_select': true,
    'options': [
      {'label': 'Distance'},
      {'label': 'Time', 'description': 'Moving time, not stops'},
      {'label': 'Climb'},
    ],
  });
  api.placeQuestionInThread(threaded, 'thread-1');
  await ask(const {
    'question': 'What should the empty list say?',
    'header': 'Empty list',
  });
  final nav = GlobalKey<NavigatorState>();
  await s.tester.pumpWidget(
    s.app(
      Navigator(
        key: nav,
        onGenerateRoute: (_) => MaterialPageRoute<void>(
          builder: (_) => Scaffold(
            appBar: AppBar(title: const Text('Trips')),
            body: const Center(child: Text('A page')),
          ),
        ),
      ),
      title: 'Trips',
    ),
  );
  Future<void> _ = QuestionsSheet.show(
    nav.currentContext!,
    api: api,
    artifactId: aid,
  );
  await s.settle();
  await s.tester.tap(find.text('Kilometres'));
  await s.settle(2);
  expect(find.text('Clax has 3 questions'), findsOneWidget);
  await s.take('questions');
}

void main() {
  shots('questions', _sheet, forms: const [phone, iphone, wide, mac]);
  shots('questions', _sheet, forms: const [phone], textScales: const [1.3, 2]);
}
