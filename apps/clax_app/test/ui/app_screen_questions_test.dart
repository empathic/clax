// The app screen draws Clax's questions natively: the shell posts only the
// open IDs and `raise`, and the screen opens `QuestionsSheet`, read from the
// daemon, once no composer or permission prompt is up.
import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/questions/question_card.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

const _draft = StateMsg(
  commenting: true,
  draft: Draft(label: 'h1', text: '', clip: DraftClip.pending),
  busy: false,
);
const _noDraft = StateMsg(commenting: true, draft: null, busy: false);

/// Reduced motion for the rest of the test: the questions sheet's mark
/// breathes until then, which no frame settles.
void _stillMotion(WidgetTester tester) {
  tester.platformDispatcher.accessibilityFeaturesTestValue =
      const FakeAccessibilityFeatures(disableAnimations: true);
  addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
}

void main() {
  late FakeAppSurface surface;
  late FakeClaxApi api;
  late String aid;
  late String qid;

  Future<void> pumpScreen(WidgetTester tester) async {
    tester.view.physicalSize = const Size(1080, 2340);
    tester.view.devicePixelRatio = 3;
    addTearDown(tester.view.reset);
    _stillMotion(tester);
    api = FakeClaxApi();
    aid = api.seedArtifact('Trips', const {
      'index.html': '<!doctype html><h1>Trips</h1>',
    });
    final s = await api.registerAppSession(installId: 'i', pid: 1);
    qid = (await api.askQuestion(s.id, [
      {
        'question': 'Which unit should distances use?',
        'header': 'Units',
        'options': [
          {'label': 'Kilometres'},
          {'label': 'Miles'},
        ],
      },
    ], artifactId: aid)).question.id;
    surface = FakeAppSurface();
    final worker = StreamController<WorkerEvent>.broadcast();
    addTearDown(worker.close);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, TargetPlatform.android),
        home: AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: NoTimings(),
          worker: worker.stream,
        ),
      ),
    );
    surface.push(
      Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pump();
  }

  final card = find.byType(QuestionCard);
  final question = find.text('Which unit should distances use?');

  test('the app declares the questions sheet native', () {
    expect(nativeOverlays, containsAll(['ask', 'questions']));
  });

  testWidgets('a raise opens the native sheet for the app, never the '
      'shell’s', (tester) async {
    await pumpScreen(tester);
    surface.push(QuestionsMsg(1, ids: [qid], raise: true));
    await tester.pumpAndSettle();
    expect(card, findsOneWidget);
    expect(question, findsOneWidget);
    expect(surface.calls.whereType<OpenQuestions>(), isEmpty);
    await tester.tap(find.text('Miles'));
    await tester.pump();
    await tester.tap(find.text('Answer'));
    await tester.pumpAndSettle();
    expect(card, findsNothing);
    expect(api.calls.where((c) => c.name == 'answerQuestion'), hasLength(1));
  });

  testWidgets('a raise while the composer is open waits, and opens once it '
      'closes', (tester) async {
    await pumpScreen(tester);
    surface.push(_draft);
    await tester.pumpAndSettle();
    surface.push(QuestionsMsg(1, ids: [qid], raise: true));
    await tester.pumpAndSettle();
    expect(card, findsNothing, reason: 'the composer is the person’s');
    surface.push(_noDraft);
    await tester.pumpAndSettle();
    expect(card, findsOneWidget);
  });

  testWidgets('a raise while a permission prompt is up waits for its '
      'answer', (tester) async {
    await pumpScreen(tester);
    surface.push(
      const AskMsg(
        id: 'a1',
        kind: AskKind.capability,
        title: 'Let Trips use the network?',
        body: 'Trips wants to reach maps.example.',
        allow: 'Allow',
        deny: 'Don’t allow',
        armMs: 0,
      ),
    );
    await tester.pumpAndSettle();
    surface.push(QuestionsMsg(1, ids: [qid], raise: true));
    await tester.pumpAndSettle();
    expect(card, findsNothing);
    await tester.tap(find.text('Don’t allow'));
    await tester.pumpAndSettle();
    expect(card, findsOneWidget);
  });

  testWidgets('without raise a new question is told in a toast whose View '
      'opens the sheet', (tester) async {
    await pumpScreen(tester);
    surface.push(const QuestionsMsg(0));
    await tester.pump();
    surface.push(QuestionsMsg(1, ids: [qid]));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));
    expect(card, findsNothing);
    expect(find.text('Clax has a question'), findsOneWidget);
    await tester.tap(find.text('View'));
    await tester.pumpAndSettle();
    expect(card, findsOneWidget);
    expect(surface.calls.whereType<OpenQuestions>(), isEmpty);
    // The toast's own time runs out.
    await tester.pump(const Duration(seconds: 10));
  });

  testWidgets('a raise while the sheet shows reads the questions again', (
    tester,
  ) async {
    await pumpScreen(tester);
    surface.push(QuestionsMsg(1, ids: [qid], raise: true));
    await tester.pumpAndSettle();
    final s = await api.registerAppSession(installId: 'i', pid: 2);
    final next = (await api.askQuestion(s.id, [
      {'question': 'Which day do trips start on?', 'header': 'Start'},
    ], artifactId: aid)).question.id;
    surface.push(QuestionsMsg(2, ids: [qid, next], raise: true));
    await tester.pumpAndSettle();
    expect(card, findsNWidgets(2));
  });
}
