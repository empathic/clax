import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/ai_consent.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/ai_consent_sheet.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'loop_support.dart';

void main() {
  late Directory dir;
  late SettingsStore store;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('clax-ai-consent-');
    store = SettingsStore(File('${dir.path}/settings.json'));
  });
  tearDown(() => dir.deleteSync(recursive: true));

  /// A page on an iPhone whose context the consent asks over.
  Future<BuildContext> page(WidgetTester tester) async {
    late BuildContext context;
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, TargetPlatform.iOS),
        home: Builder(
          builder: (c) {
            context = c;
            return const Scaffold(body: Text('Home'));
          },
        ),
      ),
    );
    return context;
  }

  final sheet = find.text(aiConsentTitle);

  testWidgets('without required no sheet ever shows and nothing is kept', (
    tester,
  ) async {
    final context = await page(tester);
    final consent = AiConsent(required: false, settings: store);
    expect(await consent.ensure(context), isTrue);
    await tester.pumpAndSettle();
    expect(sheet, findsNothing);
    expect((await store.load()).aiConsent, isNull);
  });

  testWidgets('the first ensure asks and answers nothing until Allow, which '
      'is kept, so the next ensure does not ask', (tester) async {
    final context = await page(tester);
    final consent = AiConsent(required: true, settings: store);
    bool? answer;
    unawaited(consent.ensure(context).then((a) => answer = a));
    await tester.pumpAndSettle();
    expect(sheet, findsOneWidget);
    expect(find.text(aiConsentBody), findsOneWidget);
    expect(answer, isNull);

    await tester.tap(find.text('Allow'));
    await tester.pumpAndSettle();
    expect(answer, isTrue);
    expect(sheet, findsNothing);
    expect((await store.load()).aiConsent, isTrue);

    expect(await consent.ensure(context), isTrue);
    await tester.pumpAndSettle();
    expect(sheet, findsNothing);
  });

  testWidgets('Not now answers false, keeps nothing, and the next ensure '
      'asks again', (tester) async {
    final context = await page(tester);
    final consent = AiConsent(required: true, settings: store);
    bool? answer;
    unawaited(consent.ensure(context).then((a) => answer = a));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Not now'));
    await tester.pumpAndSettle();
    expect(answer, isFalse);
    expect((await store.load()).aiConsent, isNot(isTrue));

    unawaited(consent.ensure(context));
    await tester.pumpAndSettle();
    expect(sheet, findsOneWidget);
  });

  testWidgets('closing the sheet without an answer is Not now', (
    tester,
  ) async {
    final context = await page(tester);
    final consent = AiConsent(required: true, settings: store);
    bool? answer;
    unawaited(consent.ensure(context).then((a) => answer = a));
    await tester.pumpAndSettle();
    Navigator.of(context).pop();
    await tester.pumpAndSettle();
    expect(answer, isFalse);
  });

  testWidgets('a consent turned off in Settings asks again', (tester) async {
    await store.save(AppSettings()..aiConsent = false);
    final context = await page(tester);
    final consent = AiConsent(required: true, settings: store);
    unawaited(consent.ensure(context));
    await tester.pumpAndSettle();
    expect(sheet, findsOneWidget);
  });

  testWidgets('runs that ask while the sheet shows share it and its answer', (
    tester,
  ) async {
    final context = await page(tester);
    final consent = AiConsent(required: true, settings: store);
    final answers = <bool>[];
    unawaited(consent.ensure(context).then(answers.add));
    await tester.pumpAndSettle();
    unawaited(consent.ensure(context).then(answers.add));
    await tester.pumpAndSettle();
    expect(sheet, findsOneWidget);
    await tester.tap(find.text('Allow'));
    await tester.pumpAndSettle();
    expect(answers, [true, true]);
  });

  group('the loop asks before a change sends anything', () {
    late FakeClaxApi api;
    late HeldBackend backend;
    late String aid;
    late String sid;
    final asked = <(String?, bool)>[];

    AgentLoop loop(bool allow) {
      backend = HeldBackend([
        change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      ]);
      return AgentLoop(
        api: api,
        singleCall: () => backend,
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: RecordingTimings(),
        gate: (aid, {required build}) async {
          asked.add((aid, build));
          return allow;
        },
      );
    }

    setUp(() async {
      asked.clear();
      api = FakeClaxApi();
      sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
      aid = api.seedArtifact('Runs', {
        'index.html': '<!doctype html><title>Runs</title><button>Save</button>',
      });
    });

    test('a refused change ends cancelled having sent nothing', () async {
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final out = await loop(false).change(aid, [item], session: sid);
      expect(out, isA<ChangeCancelled>());
      expect(backend.seen, isEmpty);
      expect(asked, [(aid, false)]);
      expect(api.callNames, isNot(contains('setWorking')));
    });

    test('an allowed change runs', () async {
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final out = await loop(true).change(aid, [item], session: sid);
      expect(out, isA<ChangeLanded>());
      expect(backend.seen, hasLength(1));
      expect(asked, [(aid, false)]);
    });

    test('a refused build run ends cancelled having sent nothing', () async {
      final out = await loop(false).continueBuild(aid);
      expect(out, isA<ChangeCancelled>());
      expect(backend.seen, isEmpty);
      expect(asked, [(aid, true)]);
    });

    test('an empty change asks nothing', () async {
      await loop(false).change(aid, const [], session: sid);
      expect(asked, isEmpty);
    });
  });

  test('copy for the sheet', () {
    expect(aiConsentTitle, 'Clax sends your request to Anthropic');
    expect(
      aiConsentBody,
      'What you type or say, and the app you’re changing, go to '
      'Anthropic’s Claude API under your own key, to make and change your '
      'apps.',
    );
  });
}
