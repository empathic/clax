import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_voice.dart';
import 'bundle_flows_test.dart' show FakeBundleFiles, settle;

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

  Future<void> pumpLauncher(WidgetTester tester, BundleFlows? bundles) async {
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
    await settle(tester);
  }

  testWidgets('long press, Share app: the export sheet, then the share '
      'sheet gets the file', (tester) async {
    await tester.runAsync(me.ready);
    me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    await pumpLauncher(tester, flows());
    await tester.longPress(find.text('Plant log'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('share-app')));
    await settle(tester);
    expect(find.byType(ExportSheet), findsOneWidget);
    await tester.tap(find.text('Share'));
    await settle(tester);
    expect(files.sent.single.$1, 'plant-log.clax');
  });

  testWidgets('on the Mac the menu says Export…', (tester) async {
    me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    await pumpLauncher(tester, flows(toFile: true));
    await tester.longPress(find.text('Plant log'));
    await tester.pumpAndSettle();
    expect(find.text('Export…'), findsOneWidget);
    expect(find.text('Share app'), findsNothing);
  });

  testWidgets('without bundles there is no Share app or Import app…', (
    tester,
  ) async {
    me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    await pumpLauncher(tester, null);
    expect(find.byKey(const ValueKey('launcher-more')), findsNothing);
    await tester.longPress(find.text('Plant log'));
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
    await pumpLauncher(tester, flows());
    await tester.tap(find.byKey(const ValueKey('launcher-more')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Import app…'));
    await settle(tester);
    expect(find.byType(InstallSheet), findsOneWidget);
    await tester.tap(find.text('Install'));
    await settle(tester);
    expect(opened, hasLength(1));
  });

  group('inspector', () {
    Future<void> pumpInspector(WidgetTester tester, String aid) async {
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
      await settle(tester);
    }

    testWidgets('Share app, and "Shared as v1" once shared', (tester) async {
      await tester.runAsync(me.ready);
      final aid = me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
      await pumpInspector(tester, aid);
      expect(find.textContaining('Shared as'), findsNothing);
      await tester.tap(find.byKey(const ValueKey('inspector-share')));
      await settle(tester);
      await tester.tap(find.text('Share'));
      await settle(tester);
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
        aid = await me.installer.install(await me.installer.open(b.bytes));
      });
      await pumpInspector(tester, aid);
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
