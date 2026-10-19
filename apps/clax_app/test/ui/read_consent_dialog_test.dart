import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/read_consent.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final nav = GlobalKey<NavigatorState>();

  Future<void> pumpApp(WidgetTester tester) => tester.pumpWidget(
    MaterialApp(
      navigatorKey: nav,
      home: const Scaffold(body: Text('home')),
    ),
  );

  DialogReadConsent consent([
    ConsentMemory? memory,
    void Function() Function(String question)? waiting,
  ]) => DialogReadConsent(
    navigator: () => nav.currentState,
    memory: memory ?? MemoryConsentMemory(),
    waiting: waiting,
  );

  /// Ticks "Always for this app".
  Future<void> always(WidgetTester tester) async {
    await tester.tap(find.text('Always for this app'));
    await tester.pump();
  }

  Future<ConsentAnswer> ask(
    DialogReadConsent c, {
    String? reader = 'a0000000000a',
    String? readerTitle = 'Dashboard',
    Future<void>? cancelled,
  }) => c.ask(
    reader: reader,
    readerTitle: readerTitle,
    source: 'b0000000000b',
    sourceTitle: 'Runs',
    cancelled: cancelled,
  );

  testWidgets('the title is one line naming Clax and the source, the body '
      'the app it works on; Don’t allow, Allow once and an Always checkbox', (
    tester,
  ) async {
    await pumpApp(tester);
    final c = consent();
    final answer = ask(c);
    await tester.pumpAndSettle();
    expect(find.text('Let Clax read Runs?'), findsOneWidget);
    expect(
      find.text(
        'Clax would use its data in Dashboard. It never changes another '
        'app’s data.',
      ),
      findsOneWidget,
    );
    expect(find.textContaining('agent'), findsNothing);
    expect(find.text('Allow once'), findsOneWidget);
    expect(find.byType(ClaxCheckbox), findsOneWidget);
    expect(
      find.widgetWithText(ClaxButton, 'Always for this app'),
      findsNothing,
    );
    expect(find.byType(ClaxButton), findsNWidgets(2));
    expect(find.text('Don’t allow'), findsOneWidget);
    await tester.tap(find.text('Allow once'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.once);
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets("Don't allow, and dismissing the dialog, deny", (tester) async {
    await pumpApp(tester);
    final c = consent();
    var answer = ask(c);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Don’t allow'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.denied);

    answer = ask(c);
    await tester.pumpAndSettle();
    await tester.tapAt(const Offset(5, 5));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.denied);
  });

  testWidgets('Always is remembered for the pair, and asked no more', (
    tester,
  ) async {
    await pumpApp(tester);
    final c = consent();
    var answer = ask(c);
    await tester.pumpAndSettle();
    await always(tester);
    // The button says what it grants.
    expect(find.text('Allow once'), findsNothing);
    await tester.tap(find.text('Always allow'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.always);

    answer = ask(c);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(await answer, ConsentAnswer.always);

    // Another app working on the same source is asked.
    answer = ask(c, reader: 'c0000000000c', readerTitle: 'Week');
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    await tester.tap(find.text('Don’t allow'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.denied);
  });

  testWidgets('unticking Always puts Allow once back', (tester) async {
    await pumpApp(tester);
    final answer = ask(consent());
    await tester.pumpAndSettle();
    await always(tester);
    expect(find.text('Always allow'), findsOneWidget);
    await always(tester);
    expect(find.text('Always allow'), findsNothing);
    await tester.tap(find.text('Allow once'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.once);
  });

  testWidgets('a new app is offered no Always', (tester) async {
    await pumpApp(tester);
    final answer = ask(consent(), reader: null, readerTitle: null);
    await tester.pumpAndSettle();
    expect(find.text('Let Clax read Runs?'), findsOneWidget);
    expect(find.textContaining('in your new app.'), findsOneWidget);
    expect(find.text('Always for this app'), findsNothing);
    await tester.tap(find.text('Allow once'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.once);
  });

  testWidgets('a cancelled run takes the dialog down and is denied', (
    tester,
  ) async {
    await pumpApp(tester);
    final cancel = Completer<void>();
    final answer = ask(consent(), cancelled: cancel.future);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    cancel.complete();
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(await answer, ConsentAnswer.denied);
    expect(find.text('home'), findsOneWidget);
  });

  testWidgets('Always ticked with Don’t allow denies and remembers nothing', (
    tester,
  ) async {
    await pumpApp(tester);
    final memory = MemoryConsentMemory();
    final answer = ask(consent(memory));
    await tester.pumpAndSettle();
    await always(tester);
    await tester.tap(find.text('Don’t allow'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.denied);
    expect(await memory.pairs(), isEmpty);
  });

  testWidgets('while the dialog waits, the run’s notification says Clax '
      'needs your OK', (tester) async {
    await pumpApp(tester);
    final log = <String>[];
    final answer = ask(
      consent(null, (q) {
        log.add(q);
        return () => log.add('answered');
      }),
    );
    await tester.pumpAndSettle();
    expect(log, ['Let Clax read Runs?']);
    await tester.tap(find.text('Allow once'));
    await tester.pumpAndSettle();
    expect(await answer, ConsentAnswer.once);
    expect(log, ['Let Clax read Runs?', 'answered']);
  });

  test('without a navigator the answer is denied', () async {
    final c = DialogReadConsent(
      navigator: () => null,
      memory: MemoryConsentMemory(),
    );
    expect(await ask(c), ConsentAnswer.denied);
  });

  test('FileConsentMemory keeps the pairs across loads and ignores a broken '
      'file', () async {
    final dir = Directory.systemTemp.createTempSync('consent-');
    addTearDown(() => dir.deleteSync(recursive: true));
    final file = File('${dir.path}/agent-read-consent.json');
    final m = FileConsentMemory(file);
    expect(await m.allowed('a', 'b'), isFalse);
    await m.allow('a', 'b');
    final again = FileConsentMemory(file);
    expect(await again.allowed('a', 'b'), isTrue);
    expect(await again.allowed('b', 'a'), isFalse);
    expect(File('${file.path}.tmp').existsSync(), isFalse);

    file.writeAsStringSync('{not json');
    expect(await FileConsentMemory(file).allowed('a', 'b'), isFalse);
  });

  test(
    'FileConsentMemory lists its pairs and forgets one across loads',
    () async {
      final dir = Directory.systemTemp.createTempSync('consent-');
      addTearDown(() => dir.deleteSync(recursive: true));
      final file = File('${dir.path}/agent-read-consent.json');
      final m = FileConsentMemory(file);
      await m.allow('a', 'b');
      await m.allow('a', 'c');
      expect(await m.pairs(), unorderedEquals([('a', 'b'), ('a', 'c')]));
      await m.forget('a', 'b');
      final again = FileConsentMemory(file);
      expect(await again.pairs(), [('a', 'c')]);
      expect(await again.allowed('a', 'b'), isFalse);
    },
  );
}
