import 'package:clax_app/clax/models.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _app = ArtifactSummary(
  id: 'a_plants',
  title: 'Plant log',
  icon: 'leaf',
  currentVersion: 4,
  updatedAt: '2026-10-13T09:00:00Z',
);

const _bundle = BundleSummary(
  lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
  title: 'Plant log',
  version: 3,
  fingerprint: '7KQ2-M9XD-4FTA-QW3B',
  name: 'Alex',
  capabilities: {
    'db': {},
    'notify': {},
    'net': {
      'hosts': ['api.weather.example'],
    },
  },
  word: 'leaf',
  seedDocs: 12,
);

/// Pumps a screen whose button shows [sheet] with [showBundleSheet]; the
/// sheet's answer lands in the returned list.
Future<List<Object?>> _show(
  WidgetTester tester,
  Widget sheet, {
  Size size = const Size(400, 800),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final answers = <Object?>[];
  await tester.pumpWidget(
    MaterialApp(
      // A new app each time, so a sheet left up does not stay.
      key: UniqueKey(),
      home: Builder(
        builder: (context) => Scaffold(
          body: Center(
            child: TextButton(
              onPressed: () async =>
                  answers.add(await showBundleSheet<Object?>(context, sheet)),
              child: const Text('show'),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('show'));
  await tester.pumpAndSettle();
  expect(tester.takeException(), isNull);
  return answers;
}

void main() {
  group('words', () {
    test('capabilities in words, hosts by name', () {
      expect(capabilityWords(_bundle.capabilities), [
        'keep its own data',
        'send you notifications and reminders',
        'use the internet: api.weather.example',
      ]);
      expect(capabilityWords({'sample': {}}), ['use Claude (on your API key)']);
    });

    test('added capabilities: new ones and new net hosts only', () {
      expect(
        addedCapabilityWords(
          {
            'db': {},
            'net': {
              'hosts': ['a.example'],
            },
          },
          {
            'db': {},
            'camera': {},
            'net': {
              'hosts': ['a.example', 'b.example'],
            },
          },
        ),
        [
          'take photos and pick photos from your library',
          'use the internet: b.example',
        ],
      );
      expect(addedCapabilityWords({'db': {}}, {'db': {}}), isEmpty);
    });

    test('refusals in one sentence', () {
      for (final code in [
        'bad_manifest',
        'bad_signature',
        'bad_file',
        'bad_seed',
      ]) {
        expect(
          refusalMessage(code),
          'This file is damaged or was changed after it was signed.',
        );
      }
      expect(
        refusalMessage('too_new'),
        'Made by a newer Clax: update Clax to open it.',
      );
      expect(refusalMessage('invalid_capabilities'), contains('newer Clax'));
    });

    test('from whom, and sizes', () {
      expect(fromLine(name: 'Alex', fingerprint: 'F'), 'From Alex · F');
      expect(fromLine(name: null, fingerprint: 'F'), 'From F');
      expect(fromLine(name: 'Alex', fingerprint: 'F', own: true), 'From you');
      expect(byteSize(512), '512 bytes');
      expect(byteSize(12345), '12 KB');
      expect(byteSize(2400000), '2.4 MB');
    });
  });

  group('ExportSheet', () {
    ExportSheet own({bool fits = true, bool askName = false}) => ExportSheet(
      app: _app,
      mode: ExportMode.own,
      version: 3,
      files: 4,
      bytes: 12345,
      docs: 7,
      dataFits: fits,
      askName: askName,
    );

    testWidgets('shows what goes; data is off unless switched on', (
      tester,
    ) async {
      final answers = await _show(tester, own());
      expect(find.text('Plant log'), findsOneWidget);
      expect(find.text('v3 · 4 files, 12 KB'), findsOneWidget);
      expect(find.text('7 documents'), findsOneWidget);
      final sw = tester.widget<SwitchListTile>(
        find.byKey(const ValueKey('include-data')),
      );
      expect(sw.value, isFalse);
      await tester.tap(find.text('Share'));
      await tester.pumpAndSettle();
      final c = answers.single! as ExportChoice;
      expect(c.original, isFalse);
      expect(c.includeData, isFalse);
      expect(c.name, isNull);
    });

    testWidgets('Include my data and a typed name go with the choice', (
      tester,
    ) async {
      final answers = await _show(tester, own(askName: true));
      await tester.tap(find.text('Include my data'));
      await tester.pump();
      await tester.enterText(
        find.byKey(const ValueKey('export-name')),
        ' Sam ',
      );
      await tester.tap(find.byKey(const ValueKey('export-own')));
      await tester.pumpAndSettle();
      final c = answers.single! as ExportChoice;
      expect(c.includeData, isTrue);
      expect(c.name, 'Sam');
    });

    testWidgets('too much data: the switch is off and says so', (tester) async {
      await _show(tester, own(fits: false));
      expect(find.text('Too much data to include'), findsOneWidget);
      final sw = tester.widget<SwitchListTile>(
        find.byKey(const ValueKey('include-data')),
      );
      expect(sw.onChanged, isNull);
      expect(find.byKey(const ValueKey('export-name')), findsNothing);
    });

    testWidgets('on the Mac it exports', (tester) async {
      await _show(
        tester,
        const ExportSheet(
          app: _app,
          mode: ExportMode.own,
          version: 1,
          files: 1,
          bytes: 10,
          docs: 0,
          dataFits: true,
          toFile: true,
        ),
        size: const Size(1200, 800),
      );
      expect(find.byType(Dialog), findsOneWidget);
      expect(find.text('Export'), findsOneWidget);
      expect(find.text('Share'), findsNothing);
    });

    testWidgets('an unmodified copy goes as received, with no data switch', (
      tester,
    ) async {
      final answers = await _show(
        tester,
        const ExportSheet(
          app: _app,
          mode: ExportMode.original,
          version: 2,
          originalVersion: 2,
          author: 'Alex',
          files: 4,
          bytes: 100,
          docs: 9,
          dataFits: true,
        ),
      );
      expect(find.text('Shares Alex’s v2 as you received it.'), findsOneWidget);
      expect(find.byKey(const ValueKey('include-data')), findsNothing);
      await tester.tap(find.text('Share'));
      await tester.pumpAndSettle();
      expect((answers.single! as ExportChoice).original, isTrue);
      expect((answers.single! as ExportChoice).includeData, isFalse);
    });

    testWidgets('a modified copy: the original, or mine as a new app', (
      tester,
    ) async {
      const sheet = ExportSheet(
        app: _app,
        mode: ExportMode.modified,
        version: 1,
        originalVersion: 2,
        author: 'Alex',
        files: 5,
        bytes: 2000,
        docs: 3,
        dataFits: true,
      );
      var answers = await _show(tester, sheet);
      expect(find.text('Share Alex’s v2'), findsOneWidget);
      await tester.tap(find.text('Share my version as a new app'));
      await tester.pumpAndSettle();
      expect((answers.single! as ExportChoice).original, isFalse);

      answers = await _show(tester, sheet);
      await tester.tap(find.text('Include my data'));
      await tester.pump();
      await tester.tap(find.text('Share Alex’s v2'));
      await tester.pumpAndSettle();
      final c = answers.single! as ExportChoice;
      expect(c.original, isTrue);
      expect(c.includeData, isFalse, reason: 'never data in their bundle');
    });
  });

  group('InstallSheet', () {
    testWidgets('who, what it can ask for, and its documents', (tester) async {
      final answers = await _show(
        tester,
        const InstallSheet(bundle: _bundle, kind: InstallKind.install),
      );
      expect(find.text('v3'), findsOneWidget);
      expect(find.text('From Alex · 7KQ2-M9XD-4FTA-QW3B'), findsOneWidget);
      expect(
        find.text('“Alex” is the sender’s own description'),
        findsOneWidget,
      );
      expect(
        find.text('•  use the internet: api.weather.example'),
        findsOneWidget,
      );
      expect(find.text('Comes with 12 documents'), findsOneWidget);
      await tester.tap(find.text('Install'));
      await tester.pumpAndSettle();
      expect(answers, [true]);
    });

    testWidgets('Cancel installs nothing', (tester) async {
      final answers = await _show(
        tester,
        const InstallSheet(bundle: _bundle, kind: InstallKind.install),
      );
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(answers, [null]);
    });

    testWidgets('someone else’s lineage installs separately', (tester) async {
      await _show(
        tester,
        const InstallSheet(bundle: _bundle, kind: InstallKind.foreign),
      );
      expect(
        find.textContaining('is already installed from someone else'),
        findsOneWidget,
      );
      expect(find.text('Install separately'), findsOneWidget);
    });

    testWidgets('the original of a fork installs separately', (tester) async {
      await _show(
        tester,
        const InstallSheet(bundle: _bundle, kind: InstallKind.forked),
      );
      expect(
        find.text(
          'You made Plant log your own. Install Alex’s v3 as a separate app?',
        ),
        findsOneWidget,
      );
    });

    testWidgets('a bundle of this install’s key is from you', (tester) async {
      await _show(
        tester,
        const InstallSheet(
          bundle: BundleSummary(
            lineage: 'l_bbbbbbbbbbbbbbbbbbbbbbbbbb',
            title: 'Mine',
            version: 1,
            fingerprint: 'F',
            name: 'Alex',
            capabilities: {},
            own: true,
          ),
          kind: InstallKind.install,
        ),
      );
      expect(find.text('From you'), findsOneWidget);
      expect(find.textContaining('own description'), findsNothing);
    });
  });

  group('UpdateSheet', () {
    testWidgets('an unmodified copy: Update or Not now, with added asks', (
      tester,
    ) async {
      var answers = await _show(
        tester,
        const UpdateSheet(
          bundle: _bundle,
          have: 2,
          added: ['take photos and pick photos from your library'],
        ),
      );
      expect(find.text('Update Plant log to v3?'), findsOneWidget);
      expect(find.textContaining('You have v2.'), findsOneWidget);
      expect(find.text('This version also asks to:'), findsOneWidget);
      await tester.tap(find.text('Update'));
      await tester.pumpAndSettle();
      expect(answers, [UpdateChoice.update]);

      answers = await _show(
        tester,
        const UpdateSheet(bundle: _bundle, have: 2, added: []),
      );
      expect(find.text('This version also asks to:'), findsNothing);
      await tester.tap(find.text('Not now'));
      await tester.pumpAndSettle();
      expect(answers, [null]);
    });

    testWidgets('a modified copy: replace the changes or keep them', (
      tester,
    ) async {
      var answers = await _show(
        tester,
        const UpdateSheet(bundle: _bundle, have: 2, added: [], modified: true),
      );
      expect(find.text('You changed Plant log since v2'), findsOneWidget);
      await tester.tap(find.text('Keep mine (stop updates for this app)'));
      await tester.pumpAndSettle();
      expect(answers, [UpdateChoice.keepMine]);

      answers = await _show(
        tester,
        const UpdateSheet(bundle: _bundle, have: 2, added: [], modified: true),
      );
      await tester.tap(find.text('Update (replaces your changes)'));
      await tester.pumpAndSettle();
      expect(answers, [UpdateChoice.update]);
    });

    testWidgets('stopped updates ask the same, saying when', (tester) async {
      await _show(
        tester,
        UpdateSheet(
          bundle: _bundle,
          have: 2,
          added: const [],
          stoppedAt: DateTime(2026, 10, 2, 12),
        ),
      );
      expect(
        find.text('You kept your changes on 2 October 2026.'),
        findsOneWidget,
      );
      expect(find.text('Update (replaces your changes)'), findsOneWidget);
    });
  });

  group('BundleProgressSheet', () {
    testWidgets('follows the check to a pass, then Open', (tester) async {
      final p = ValueNotifier<BundleProgress>(
        const BundleWorking('Checking v3 against your data…', 'Data'),
      );
      final answers = await _showWorking(tester, p);
      expect(find.text('Checking v3 against your data…'), findsOneWidget);
      expect(find.text('Data'), findsOneWidget);
      // Working: a tap outside does not take it down.
      await tester.tapAt(const Offset(10, 10));
      await tester.pump();
      expect(find.text('Data'), findsOneWidget);
      p.value = const BundleWorking('Checking v3 against your data…', 'Tests');
      await tester.pump();
      expect(find.text('Tests'), findsOneWidget);
      p.value = const BundlePassed('Updated to v3');
      await tester.pump();
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(answers, [true]);
    });

    testWidgets('a failure says the app is unchanged, with Details', (
      tester,
    ) async {
      final p = ValueNotifier<BundleProgress>(
        updateCheckFailed(
          3,
          'a test failed',
          details: ['adds a plant: expected 1, got 0'],
        ),
      );
      final answers = await _showWorking(tester, p);
      expect(
        find.text(
          "v3 didn't pass its checks on your data: a test failed. "
          'Your app is unchanged.',
        ),
        findsOneWidget,
      );
      expect(find.text('adds a plant: expected 1, got 0'), findsNothing);
      await tester.tap(find.text('Details'));
      await tester.pump();
      expect(find.text('adds a plant: expected 1, got 0'), findsOneWidget);
      await tester.tap(find.text('Keep current'));
      await tester.pumpAndSettle();
      expect(answers, [null]);
    });
  });

  group('BundleNoticeSheet', () {
    testWidgets('a refusal: one sentence and Close', (tester) async {
      final answers = await _show(
        tester,
        BundleNoticeSheet.refused('bad_signature'),
      );
      expect(
        find.text('This file is damaged or was changed after it was signed.'),
        findsOneWidget,
      );
      expect(find.text('Open'), findsNothing);
      await tester.tap(find.text('Close'));
      await tester.pumpAndSettle();
      expect(answers, [null]);
    });

    testWidgets('own and current offer Open; older and conflict do not', (
      tester,
    ) async {
      var answers = await _show(tester, BundleNoticeSheet.own('Plant log'));
      expect(find.text('This is your app Plant log.'), findsOneWidget);
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(answers, [true]);
      answers = await _show(tester, BundleNoticeSheet.current('Plant log', 3));
      expect(find.text('You already have v3.'), findsOneWidget);
      await _show(tester, BundleNoticeSheet.older('Plant log', 2, 3));
      expect(find.textContaining('Older versions are not installed'), findsOne);
      expect(find.text('Open'), findsNothing);
      await _show(tester, BundleNoticeSheet.conflict('Plant log', 3));
      expect(find.textContaining('It was not installed.'), findsOneWidget);
    });
  });

  testWidgets('every sheet fits a small phone and a desktop window', (
    tester,
  ) async {
    for (final size in const [Size(320, 568), Size(1280, 800)]) {
      for (final sheet in <Widget>[
        const ExportSheet(
          app: _app,
          mode: ExportMode.modified,
          version: 1,
          originalVersion: 12,
          author: 'Alexandra Longname-Example',
          files: 40,
          bytes: 2400000,
          docs: 300,
          dataFits: true,
          askName: true,
        ),
        const InstallSheet(bundle: _bundle, kind: InstallKind.foreign),
        const UpdateSheet(
          bundle: _bundle,
          have: 2,
          added: ['use Claude (on your API key)'],
          modified: true,
        ),
      ]) {
        await _show(tester, sheet, size: size);
        expect(tester.takeException(), isNull);
      }
    }
  });
}

Future<List<Object?>> _showWorking(
  WidgetTester tester,
  ValueNotifier<BundleProgress> p,
) async {
  final answers = <Object?>[];
  await tester.pumpWidget(
    MaterialApp(
      home: Builder(
        builder: (context) => Scaffold(
          body: Center(
            child: TextButton(
              onPressed: () async => answers.add(
                await showBundleSheet<bool>(
                  context,
                  BundleProgressSheet(progress: p),
                  dismissible: false,
                ),
              ),
              child: const Text('show'),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('show'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 500));
  return answers;
}
