import 'package:clax_app/platform/voice.dart';
import 'package:clax_app/ui/new_sheet.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

void main() {
  late List<String?> results;

  setUp(() => results = []);

  Future<void> pumpSheet(
    WidgetTester tester,
    VoiceInput voice, {
    String initial = '',
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: TextButton(
                onPressed: () async =>
                    results.add(await showNewSheet(context, voice, initial)),
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
  }

  String fieldText(WidgetTester tester) =>
      tester.widget<TextField>(find.byType(TextField)).controller!.text;

  testWidgets('the sheet listens at once and the field shows the final words', (
    tester,
  ) async {
    final voice = FakeVoice(script: ['a run', 'a run tracker']);
    await pumpSheet(tester, voice);
    expect(voice.listens, 1);
    expect(fieldText(tester), 'a run tracker');
    expect(voice.listening, isFalse);
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(results, ['a run tracker']);
  });

  testWidgets('partial words show while listening; the mic stops it', (
    tester,
  ) async {
    final voice = FakeVoice(script: ['a run', 'a run tracker'], hold: true);
    await pumpSheet(tester, voice);
    expect(fieldText(tester), 'a run tracker');
    expect(find.text('Listening…'), findsOneWidget);
    expect(voice.listening, isTrue);
    await tester.tap(find.byTooltip('Stop listening'));
    await tester.pumpAndSettle();
    expect(voice.stops, 1);
    expect(voice.listening, isFalse);
    expect(find.text('Listening…'), findsNothing);
    expect(fieldText(tester), 'a run tracker');
  });

  testWidgets('dictation appends to text already typed', (tester) async {
    final voice = FakeVoice(script: ['with a weekly total']);
    await pumpSheet(tester, voice, initial: 'a run tracker');
    // A sheet opened with text waits for the mic instead of listening.
    expect(voice.listens, 0);
    await tester.tap(find.byTooltip('Speak'));
    await tester.pumpAndSettle();
    expect(fieldText(tester), 'a run tracker with a weekly total');
  });

  testWidgets('typing stops a running dictation', (tester) async {
    final voice = FakeVoice(script: ['a run'], hold: true);
    await pumpSheet(tester, voice);
    await tester.enterText(find.byType(TextField), 'a run log');
    await tester.pumpAndSettle();
    expect(voice.stops, 1);
    expect(fieldText(tester), 'a run log');
  });

  testWidgets('a denied voice shows why, links to settings, keeps typing', (
    tester,
  ) async {
    final voice = FakeVoice.denied('Microphone access is off');
    await pumpSheet(tester, voice);
    expect(voice.listens, 0);
    expect(find.text('Microphone access is off'), findsOneWidget);
    final mic = tester.widget<IconButton>(
      find.byKey(const Key('new-sheet-mic')),
    );
    expect(mic.onPressed, isNull);
    await tester.tap(find.text('Open settings'));
    await tester.pumpAndSettle();
    expect(voice.settingsOpened, 1);
    // Back from settings with access granted: the mic works.
    voice.availabilityNow = const VoiceAvailable();
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();
    expect(find.text('Microphone access is off'), findsNothing);
    expect(
      tester
          .widget<IconButton>(find.byKey(const Key('new-sheet-mic')))
          .onPressed,
      isNotNull,
    );
    // Typing still works.
    await tester.enterText(find.byType(TextField), 'a timer');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(results, ['a timer']);
  });

  testWidgets('an unsupported voice shows why, with no settings link', (
    tester,
  ) async {
    await pumpSheet(tester, FakeVoice.unsupported('No speech recognizer'));
    expect(find.text('No speech recognizer'), findsOneWidget);
    expect(find.text('Open settings'), findsNothing);
  });

  testWidgets('Make it is disabled until there is text', (tester) async {
    await pumpSheet(tester, FakeVoice.unsupported('No speech'));
    final make = tester.widget<FilledButton>(
      find.ancestor(
        of: find.text('Make it'),
        matching: find.byType(FilledButton),
      ),
    );
    expect(make.onPressed, isNull);
  });

  testWidgets('closing the sheet keeps nothing and stops listening', (
    tester,
  ) async {
    final voice = FakeVoice(script: ['a run'], hold: true);
    await pumpSheet(tester, voice);
    await tester.tapAt(const Offset(10, 10));
    await tester.pumpAndSettle();
    expect(results, [null]);
    expect(voice.listening, isFalse);
  });

  testWidgets('the sheet fits a small phone with long text and a reason', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(320, 568);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await pumpSheet(
      tester,
      FakeVoice.denied('Microphone or speech recognition access is turned off'),
      initial: List.filled(40, 'a long request').join(' '),
    );
    expect(tester.takeException(), isNull);
    expect(find.text('Make it'), findsOneWidget);
  });
}
