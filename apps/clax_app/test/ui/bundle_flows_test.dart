import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';

/// [BundleFiles] that keeps what it is sent and hands over [picked].
class FakeBundleFiles implements BundleFiles {
  final sent = <(String, Uint8List)>[];
  Uint8List? picked;

  @override
  Future<bool> send(String name, Uint8List bytes) async {
    sent.add((name, bytes));
    return true;
  }

  @override
  Future<Uint8List?> pick() async => picked;
}

void main() {
  late Directory tmp;
  late Install me;
  late FakeBundleFiles files;
  late SettingsStore settings;
  late List<String> opened;
  late BundleFlows flows;

  setUp(() {
    tmp = Directory.systemTemp.createTempSync('bu-flows');
    me = Install(tmp);
    files = FakeBundleFiles();
    settings = SettingsStore(File('${tmp.path}/settings.json'));
    opened = [];
    flows = BundleFlows(
      exporter: me.exporter,
      installer: me.installer,
      updater: me.updater,
      files: files,
      settings: settings,
      open: opened.add,
    );
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  /// Pumps a screen whose button runs [flow]; returns once the first
  /// sheet is up.
  Future<void> run(
    WidgetTester tester,
    Future<void> Function(BuildContext context) flow,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        key: UniqueKey(),
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: TextButton(
                onPressed: () => flow(context),
                child: const Text('go'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('go'));
    await settle(tester);
  }

  testWidgets('Share app: the sheet, then the bundle to the share sheet, '
      'with the typed name kept', (tester) async {
    late String aid;
    await tester.runAsync(() async {
      await me.ready();
      aid = (await me.api.createArtifact(
        PublishRequest(title: 'Plant log', files: {'index.html': '<p>hi</p>'}),
      )).artifact.id;
    });
    await run(tester, (c) => flows.share(c, aid));
    expect(find.text('v1 · 1 file, 9 bytes'), findsOneWidget);
    await tester.enterText(find.byKey(const ValueKey('export-name')), 'Alex');
    await tester.tap(find.text('Share'));
    await settle(tester);
    expect(files.sent, hasLength(1));
    final (name, bytes) = files.sent.single;
    expect(name, 'plant-log.clax');
    await tester.runAsync(() async {
      final b = await BundleReader.read(bytes);
      expect(b.manifest.author.name, 'Alex');
      expect(b.seed, isNull);
      expect((await settings.load()).authorName, 'Alex');
      expect((await me.store.entry(aid))?.role, LineageRole.author);
    });
    expect(find.byType(BundleProgressSheet), findsNothing);
  });

  testWidgets('a bundle opened: the install sheet, then the app opens', (
    tester,
  ) async {
    late Uint8List bytes;
    await tester.runAsync(() async {
      await me.ready();
      bytes = (await makeBundle(lineage: lineageA)).bytes;
    });
    await run(tester, (c) => flows.openBytes(c, bytes));
    expect(find.byType(InstallSheet), findsOneWidget);
    expect(find.textContaining('From Sam · '), findsOneWidget);
    await tester.tap(find.text('Install'));
    await settle(tester);
    expect(opened, hasLength(1));
    expect(find.byType(BundleProgressSheet), findsNothing);
    await tester.runAsync(() async {
      expect((await me.store.entry(opened.single))?.version, 1);
    });
  });

  testWidgets('an update: checked, landed, then Open', (tester) async {
    late String aid;
    late Uint8List v2;
    await tester.runAsync(() async {
      await me.ready();
      final v1 = await makeBundle(lineage: lineageA);
      aid = await me.installer.install(await me.installer.open(v1.bytes));
      v2 = (await makeBundle(
        lineage: lineageA,
        version: 2,
        capabilities: {'db': {}, 'notify': {}},
        files: {'index.html': '<p>v2</p>'},
      )).bytes;
    });
    await run(tester, (c) => flows.openBytes(c, v2));
    expect(find.text('Update Plant log to v2?'), findsOneWidget);
    expect(find.text('•  send you notifications and reminders'), findsOne);
    await tester.tap(find.text('Update'));
    await settle(tester);
    expect(find.text('Updated to v2'), findsOneWidget);
    await tester.tap(find.text('Open'));
    await settle(tester);
    expect(opened, [aid]);
  });

  testWidgets('a modified copy: Keep mine stops updates', (tester) async {
    late String aid;
    late Uint8List v2;
    await tester.runAsync(() async {
      await me.ready();
      final v1 = await makeBundle(lineage: lineageA);
      aid = await me.installer.install(await me.installer.open(v1.bytes));
      await me.api.publish(
        aid,
        PublishRequest(files: {'index.html': '<p>mine</p>'}, ifVersion: 1),
      );
      v2 = (await makeBundle(lineage: lineageA, version: 2)).bytes;
    });
    await run(tester, (c) => flows.openBytes(c, v2));
    expect(find.text('You changed Plant log since v1'), findsOneWidget);
    await tester.tap(find.text('Keep mine (stop updates for this app)'));
    await settle(tester);
    await tester.runAsync(() async {
      expect((await me.store.entry(aid))?.updatesStopped, isTrue);
    });
    expect(opened, isEmpty);
  });

  testWidgets('a damaged file: one sentence and Close', (tester) async {
    await run(tester, (c) => flows.openBytes(c, Uint8List.fromList([1, 2, 3])));
    expect(find.text("Couldn't open this file"), findsOneWidget);
    await tester.tap(find.text('Close'));
    await settle(tester);
    expect(find.byType(BundleNoticeSheet), findsNothing);
  });

  testWidgets('a platform refusal shows its sentence', (tester) async {
    await run(tester, (c) => flows.openFile(c, refused: 'too_large'));
    expect(find.text(refusalMessage('too_large')), findsOneWidget);
  });

  testWidgets('a file from the platform is read, and deleted when it is '
      'Clax’s own copy', (tester) async {
    late File f;
    await tester.runAsync(() async {
      await me.ready();
      f = File('${tmp.path}/in.clax')
        ..writeAsBytesSync((await makeBundle(lineage: lineageA)).bytes);
    });
    await run(tester, (c) => flows.openFile(c, path: f.path, delete: true));
    expect(find.byType(InstallSheet), findsOneWidget);
    expect(f.existsSync(), isFalse);
  });
}

/// Lets the services' real IO and crypto run, pumping between turns.
Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 20; i++) {
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pump(const Duration(milliseconds: 50));
  }
  await tester.pumpAndSettle();
}
