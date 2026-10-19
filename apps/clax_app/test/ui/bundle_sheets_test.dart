import 'package:clax_app/bundle/exporter.dart' show ExportMode, ExportPlan;
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/updater.dart' show CapabilityDelta;
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/bundle/updater.dart' show UpdateStep;
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

const _app = ArtifactSummary(
  id: 'a_plants',
  title: 'Plant log',
  icon: 'leaf',
  currentVersion: 4,
  updatedAt: '2026-10-13T09:00:00Z',
);

/// An export plan for [_app]: [modes] as the exporter gives them, and an
/// installed lineage entry by Alex at [had] when given.
ExportPlan _plan({
  List<ExportMode> modes = const [ExportMode.own],
  int version = 3,
  int files = 4,
  int bytes = 12345,
  int docs = 7,
  bool fits = true,
  int? had,
  String name = 'Alex',
}) => ExportPlan(
  aid: _app.id,
  title: _app.title,
  iconWord: _app.icon,
  modes: modes,
  version: version,
  fileCount: files,
  fileBytes: bytes,
  docCount: docs,
  dataAllowed: fits,
  modified: modes.contains(ExportMode.fork),
  entry: had == null
      ? null
      : LineageEntry(
          lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
          role: LineageRole.installed,
          key: 'A' * 43,
          name: name,
          version: had,
          released: const ['sha256:x'],
          capabilities: const {},
          title: _app.title,
        ),
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
          const CapabilityDelta(capabilities: ['camera'], hosts: ['b.example']),
        ),
        [
          'take photos and pick photos from your library',
          'use the internet: b.example',
        ],
      );
      expect(
        addedCapabilityWords(
          const CapabilityDelta(capabilities: ['net'], hosts: ['c.example']),
        ),
        ['use the internet: c.example'],
      );
      expect(
        addedCapabilityWords(
          const CapabilityDelta(capabilities: [], hosts: []),
        ),
        isEmpty,
      );
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
    ExportSheet own({bool fits = true, bool askName = false, int docs = 7}) =>
        ExportSheet(
          plan: _plan(fits: fits, docs: docs),
          askName: askName,
        );

    testWidgets('shows what goes; Share sends no data, and nothing sits '
        'above the buttons to include it', (tester) async {
      final answers = await _show(tester, own());
      expect(find.text('Plant log'), findsOneWidget);
      expect(find.text('v3 · 4 files, 12 KB'), findsOneWidget);
      expect(find.text('Include my data'), findsNothing);
      expect(find.byType(Switch), findsNothing);
      expect(
        find.text(
          'Share sends it without your data. “Share a copy with my data” '
          'adds its 7 documents.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Share'));
      await tester.pumpAndSettle();
      final c = answers.single! as ExportChoice;
      expect(c.mode, ExportMode.own);
      expect(c.includeData, isFalse);
      expect(c.name, isNull);
    });

    testWidgets('Share a copy with my data sends the documents, with a '
        'typed name', (tester) async {
      final answers = await _show(tester, own(askName: true));
      await tester.enterText(
        find.byKey(const ValueKey('export-name')),
        ' Sam ',
      );
      await tester.tap(find.text('Share a copy with my data'));
      await tester.pumpAndSettle();
      final c = answers.single! as ExportChoice;
      expect(c.mode, ExportMode.own);
      expect(c.includeData, isTrue);
      expect(c.name, 'Sam');
    });

    testWidgets('too much data, or none: no data button', (tester) async {
      await _show(tester, own(fits: false));
      expect(
        find.text('It goes without your data: there is too much to include.'),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('export-own-data')), findsNothing);
      expect(find.byKey(const ValueKey('export-name')), findsNothing);
      await _show(tester, own(docs: 0));
      expect(find.byKey(const ValueKey('export-own-data')), findsNothing);
      expect(find.textContaining('too much'), findsNothing);
    });

    testWidgets('on the Mac it exports', (tester) async {
      await _show(
        tester,
        ExportSheet(plan: _plan(version: 1), toFile: true),
        size: const Size(1200, 800),
      );
      expect(find.byType(Dialog), findsOneWidget);
      expect(find.text('Export'), findsOneWidget);
      expect(find.text('Export a copy with my data'), findsOneWidget);
      expect(find.text('Share'), findsNothing);
    });

    testWidgets('an unmodified copy goes as received, with no data button', (
      tester,
    ) async {
      final answers = await _show(
        tester,
        ExportSheet(
          plan: _plan(modes: const [ExportMode.reshare], version: 2, had: 2),
          askName: true,
        ),
      );
      expect(
        find.text('Shares Alex’s version 2 as you received it.'),
        findsOneWidget,
      );
      expect(find.textContaining('with my data'), findsNothing);
      await tester.tap(find.text('Share'));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('export-name')), findsNothing);
      expect((answers.single! as ExportChoice).mode, ExportMode.reshare);
      expect((answers.single! as ExportChoice).includeData, isFalse);
    });

    testWidgets('a modified copy: the original, or mine as a new app, with '
        'or without my data, in labels of three words at most', (tester) async {
      final sheet = ExportSheet(
        plan: _plan(
          modes: const [ExportMode.reshare, ExportMode.fork],
          version: 1,
          had: 2,
        ),
      );
      var answers = await _show(tester, sheet);
      expect(find.text('Shared as a new app · 4 files, 12 KB'), findsOneWidget);
      expect(
        find.text(
          'You changed Plant log since Alex’s version 2. “Share theirs” sends '
          'Alex’s version 2 as you received it; “Share mine” sends yours as '
          'a new app.',
        ),
        findsOneWidget,
      );
      expect(find.textContaining(RegExp(r'\bv\d')), findsNothing);
      for (final b in tester.widgetList<ClaxButton>(find.byType(ClaxButton))) {
        expect(
          b.label.split(' ').length,
          lessThanOrEqualTo(3),
          reason: b.label,
        );
      }
      await tester.tap(find.text('Share mine'));
      await tester.pumpAndSettle();
      var c = answers.single! as ExportChoice;
      expect((c.mode, c.includeData), (ExportMode.fork, false));

      answers = await _show(tester, sheet);
      await tester.tap(find.text('With my data'));
      await tester.pumpAndSettle();
      c = answers.single! as ExportChoice;
      expect((c.mode, c.includeData), (ExportMode.fork, true));

      answers = await _show(tester, sheet);
      await tester.tap(find.text('Share theirs'));
      await tester.pumpAndSettle();
      c = answers.single! as ExportChoice;
      expect(c.mode, ExportMode.reshare);
      expect(c.includeData, isFalse, reason: 'never data in their bundle');
    });

    for (final size in const [Size(320, 640), Size(412, 915), Size(1280, 800)]) {
      testWidgets('at ${size.width.round()} wide its buttons sit in one row '
          'or stack full width, never in two rows', (tester) async {
        for (final sheet in [
          own(),
          ExportSheet(
            plan: _plan(
              modes: const [ExportMode.reshare, ExportMode.fork],
              version: 1,
              had: 2,
            ),
          ),
        ]) {
          await _show(tester, sheet, size: size);
          final rects = [
            for (final e in find.byType(ClaxButton).evaluate())
              tester.getRect(find.byWidget(e.widget)),
          ];
          expect(rects.length, greaterThanOrEqualTo(3));
          final row = rects.every((r) => r.top == rects.first.top);
          final stacked =
              rects.map((r) => r.top).toSet().length == rects.length &&
              rects.every((r) => r.width == rects.first.width);
          expect(row || stacked, isTrue, reason: '$rects');
        }
      });
    }
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

    testWidgets('wide, Return presses Install, the default', (tester) async {
      final answers = await _show(
        tester,
        const InstallSheet(bundle: _bundle, kind: InstallKind.install),
        size: const Size(1280, 800),
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
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

    testWidgets('a modified copy: Keep mine or Update, the body saying '
        'what each does', (tester) async {
      var answers = await _show(
        tester,
        const UpdateSheet(bundle: _bundle, have: 2, added: [], modified: true),
      );
      expect(find.text('You changed Plant log since v2'), findsOneWidget);
      expect(
        find.text(
          'Update replaces your changes with v3. Keep mine keeps your '
          'version and stops updates for this app.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Keep mine'));
      await tester.pumpAndSettle();
      expect(answers, [UpdateChoice.keepMine]);

      answers = await _show(
        tester,
        const UpdateSheet(
          bundle: _bundle,
          have: 2,
          added: [],
          modified: true,
          keepStopsUpdates: false,
        ),
      );
      expect(
        find.textContaining('skips this update; the next one asks again.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Update'));
      await tester.pumpAndSettle();
      expect(answers, [UpdateChoice.update]);
    });

    testWidgets('Keep mine and Update sit side by side on a phone, and '
        'stack full width when they do not fit', (tester) async {
      const sheet = UpdateSheet(
        bundle: _bundle,
        have: 2,
        added: [],
        modified: true,
      );
      Rect at(String key) => tester.getRect(find.byKey(ValueKey(key)));
      await _show(tester, sheet, size: const Size(412, 915));
      expect(at('keep-mine').top, at('update').top);
      expect(at('keep-mine').right, lessThan(at('update').left));

      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _show(tester, sheet, size: const Size(320, 640));
      expect(at('update').bottom, lessThanOrEqualTo(at('keep-mine').top));
      expect(at('update').width, at('keep-mine').width);
      expect(at('update').width, 320 - 2 * 24);
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
      expect(find.text('Keep mine'), findsOneWidget);
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
      p.value = BundleWorking(
        'Checking v3 against your data…',
        updateStepLabel(UpdateStep.tests),
      );
      await tester.pump();
      expect(find.text('Running its checks'), findsOneWidget);
      expect(find.textContaining('test'), findsNothing);
      p.value = const BundlePassed('Updated to v3');
      await tester.pump();
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(answers, [true]);
    });

    testWidgets('while working it offers Cancel, which asks once and says '
        'Cancelling…; at the publish it cannot cancel', (tester) async {
      final p = ValueNotifier<BundleProgress>(
        const BundleWorking('Checking v3 against your data…', 'Copying'),
      );
      var asked = 0;
      await _showWorking(tester, p, onCancel: () => asked++);
      final cancel = find.byKey(const ValueKey('cancel'));
      expect(cancel, findsOneWidget);
      await tester.tap(cancel);
      await tester.pump();
      expect(asked, 1);
      expect(find.text('Cancelling…'), findsOneWidget);
      expect(tester.widget<ClaxButton>(cancel).onPressed, isNull);

      p.value = const BundleWorking('Checking…', 'Publishing', false);
      await _showWorking(tester, p, onCancel: () => asked++);
      expect(tester.widget<ClaxButton>(cancel).onPressed, isNull);
    });

    testWidgets('a failure says the app is unchanged, with Details as a '
        'disclosure', (tester) async {
      final p = ValueNotifier<BundleProgress>(
        updateCheckFailed(
          3,
          'a check failed',
          details: ['adds a plant: expected 1, got 0'],
        ),
      );
      final answers = await _showWorking(tester, p);
      expect(
        find.text(
          'v3 didn’t pass its checks on your data: a check failed. '
          'Your app is unchanged.',
        ),
        findsOneWidget,
      );
      expect(find.widgetWithText(ClaxButton, 'Details'), findsNothing);
      expect(find.byType(ClaxButton), findsOneWidget);
      expect(find.text('adds a plant: expected 1, got 0'), findsNothing);
      await tester.tap(find.byKey(const ValueKey('details-toggle')));
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

  testWidgets('KeptUpdateSheet: Restore pops true, Discard false', (
    tester,
  ) async {
    var answers = await _show(
      tester,
      const KeptUpdateSheet(title: 'Plant log'),
    );
    expect(find.textContaining("Plant log's update did not finish"), findsOne);
    await tester.tap(find.text('Restore'));
    await tester.pumpAndSettle();
    expect(answers, [true]);
    answers = await _show(tester, const KeptUpdateSheet(title: 'Plant log'));
    await tester.tap(find.text('Discard'));
    await tester.pumpAndSettle();
    expect(answers, [false]);
  });

  testWidgets('every sheet fits a small phone and a desktop window', (
    tester,
  ) async {
    for (final size in const [Size(320, 568), Size(1280, 800)]) {
      for (final sheet in <Widget>[
        ExportSheet(
          plan: _plan(
            modes: const [ExportMode.reshare, ExportMode.fork],
            version: 1,
            had: 12,
            name: 'Alexandra Longname-Example',
            files: 40,
            bytes: 2400000,
            docs: 300,
          ),
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
  ValueNotifier<BundleProgress> p, {
  VoidCallback? onCancel,
}) async {
  final answers = <Object?>[];
  await tester.pumpWidget(
    MaterialApp(
      key: UniqueKey(),
      home: Builder(
        builder: (context) => Scaffold(
          body: Center(
            child: TextButton(
              onPressed: () async => answers.add(
                await showBundleSheet<bool>(
                  context,
                  BundleProgressSheet(progress: p, onCancel: onCancel),
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
