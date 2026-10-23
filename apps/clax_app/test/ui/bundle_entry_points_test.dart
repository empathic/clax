import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/tile_label.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_voice.dart';
import '../support/settle.dart';
import 'bundle_flows_test.dart' show FakeBundleFiles;

/// The launcher tile titled [title], found by its label.
Finder _tile(String title) =>
    find.byWidgetPredicate((w) => w is TileLabel && w.text == title);

/// The export sheet once it has read the app: its version, files and
/// size.
final _exportReady = find.text('v1 · 1 file, 9 bytes');

void main() {
  late Directory tmp;
  late Install me;
  late FakeBundleFiles files;
  late List<String> opened;

  setUp(() {
    tmp = Directory.systemTemp.createTempSync('bu-entry');
    me = Install(tmp);
    files = FakeBundleFiles();
    opened = [];
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  BundleFlows flows({bool toFile = false}) => BundleFlows(
    exporter: me.exporter,
    installer: me.installer,
    updater: me.updater,
    files: files,
    settings: SettingsStore(File('${tmp.path}/settings.json')),
    open: opened.add,
    toFile: toFile,
  );

  /// Pumps the launcher with [bundles]; returns once it [shows].
  Future<void> pumpLauncher(
    WidgetTester tester,
    BundleFlows? bundles, {
    required Finder shows,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Launcher(
          api: me.api,
          open: opened.add,
          make: (r, c, s) => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
          bundles: bundles,
          lineage: me.store.entry,
        ),
      ),
    );
    await settle(tester, shows);
  }

  testWidgets('long press, Share app: the export sheet, then the share '
      'sheet gets the file', (tester) async {
    await tester.runAsync(me.ready);
    me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    await pumpLauncher(tester, flows(), shows: _tile('Plant log'));
    await tester.longPress(_tile('Plant log'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('share-app')));
    await settle(tester, _exportReady);
    expect(find.byType(ExportSheet), findsOneWidget);
    await tester.tap(find.text('Share'));
    await settleUntil(tester, () => files.sent.isNotEmpty);
    expect(files.sent.single.$1, 'plant-log.clax');
  });

  testWidgets('on the Mac the menu says Export…', (tester) async {
    me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    await pumpLauncher(
      tester,
      flows(toFile: true),
      shows: _tile('Plant log'),
    );
    await tester.longPress(_tile('Plant log'));
    await tester.pumpAndSettle();
    expect(find.text('Export…'), findsOneWidget);
    expect(find.text('Share app'), findsNothing);
  });

  testWidgets('without bundles there is no Share app or Import app…', (
    tester,
  ) async {
    me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    await pumpLauncher(tester, null, shows: _tile('Plant log'));
    expect(find.byKey(const ValueKey('launcher-more')), findsNothing);
    await tester.longPress(_tile('Plant log'));
    await tester.pumpAndSettle();
    expect(find.text('Share app'), findsNothing);
  });

  testWidgets('Import app… opens the picked file in the install sheet', (
    tester,
  ) async {
    late Uint8List bytes;
    await tester.runAsync(() async {
      await me.ready();
      bytes = (await makeBundle(lineage: lineageA)).bytes;
    });
    files.picked = bytes;
    await pumpLauncher(
      tester,
      flows(),
      shows: find.byKey(const ValueKey('launcher-empty')),
    );
    await tester.tap(find.byKey(const ValueKey('launcher-more')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Import app…'));
    await settle(tester, find.byType(InstallSheet));
    expect(find.byType(InstallSheet), findsOneWidget);
    await tester.tap(find.text('Install'));
    await settleUntil(tester, () => opened.isNotEmpty);
    expect(opened, hasLength(1));
  });

  group('inspector', () {
    /// Pumps [aid]'s inspector; returns once it [shows].
    Future<void> pumpInspector(
      WidgetTester tester,
      String aid, {
      required Finder shows,
    }) async {
      await tester.pumpWidget(
        MaterialApp(
          home: AppInspector(
            api: me.api,
            aid: aid,
            bundles: flows(),
            lineage: me.store.entry,
          ),
        ),
      );
      await settle(tester, shows);
    }

    testWidgets('Share app, and "Shared as v1" once shared', (tester) async {
      await tester.runAsync(me.ready);
      final aid = me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
      await pumpInspector(
        tester,
        aid,
        shows: find.byKey(const ValueKey('inspector-share')),
      );
      expect(find.textContaining('Shared as'), findsNothing);
      await tester.tap(find.byKey(const ValueKey('inspector-share')));
      await settle(tester, _exportReady);
      await tester.tap(find.text('Share'));
      await settleUntil(
        tester,
        () =>
            files.sent.isNotEmpty &&
            find.text('Shared as v1').evaluate().isNotEmpty,
      );
      expect(files.sent, hasLength(1));
      expect(find.text('Shared as v1'), findsOneWidget);
    });

    testWidgets('an installed app says from whom, with the fingerprint', (
      tester,
    ) async {
      late String aid;
      await tester.runAsync(() async {
        await me.ready();
        final b = await makeBundle(lineage: lineageA);
        aid = (await me.installer.install(await me.installer.open(b.bytes))).aid;
      });
      await pumpInspector(
        tester,
        aid,
        shows: find.textContaining('From Sam, v1 · '),
      );
      final e = (await tester.runAsync(() => me.store.entry(aid)))!;
      expect(sharingLine(e), startsWith('From Sam, v1 · '));
      expect(find.text(sharingLine(e)!), findsOneWidget);
    });
  });

  test('sharingLine', () {
    const e = LineageEntry(
      lineage: lineageA,
      role: LineageRole.installed,
      key: 'k',
      name: null,
      version: 2,
      released: [],
      capabilities: {},
      title: 't',
    );
    expect(sharingLine(null), isNull);
    expect(sharingLine(e), 'From k, v2');
    expect(sharingLine(e.copyWith(role: LineageRole.author)), 'Shared as v2');
  });
}
