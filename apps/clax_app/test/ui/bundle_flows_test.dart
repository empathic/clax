import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/platform/share_anchor.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';

/// [BundleFiles] that keeps what it is sent and hands over [picked].
class FakeBundleFiles implements BundleFiles {
  final sent = <(String, Uint8List)>[];
  final origins = <Rect>[];
  Uint8List? picked;

  @override
  Future<bool> send(String name, Uint8List bytes, {required Rect origin}) async {
    sent.add((name, bytes));
    origins.add(origin);
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
    Future<void> Function(BuildContext context) flow, {
    bool developer = false,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        key: UniqueKey(),
        builder: (_, child) => DeveloperScope(
          notifier: ValueNotifier(developer),
          child: child!,
        ),
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

  testWidgets('Share app anchors the share sheet to the tapped button, or '
      'the screen\'s centre without one', (tester) async {
    late String aid;
    await tester.runAsync(() async {
      await me.ready();
      aid = (await me.api.createArtifact(
        PublishRequest(title: 'Plant log', files: {'index.html': '<p>hi</p>'}),
      )).artifact.id;
    });
    const button = ValueKey('share-button');
    await tester.pumpWidget(
      MaterialApp(
        builder: (_, child) =>
            DeveloperScope(notifier: ValueNotifier(false), child: child!),
        home: Scaffold(
          body: Align(
            alignment: Alignment.topRight,
            child: Builder(
              builder: (b) => TextButton(
                key: button,
                onPressed: () => flows.share(b, aid, origin: shareAnchorOf(b)),
                child: const Text('go'),
              ),
            ),
          ),
        ),
      ),
    );
    final rect = tester.getRect(find.byKey(button));
    await tester.tap(find.byKey(button));
    await settle(tester);
    await tester.enterText(find.byKey(const ValueKey('export-name')), 'Alex');
    await tester.tap(find.text('Share'));
    await settle(tester);
    expect(files.origins, [rect]);

    await run(tester, (c) => flows.share(c, aid));
    await tester.tap(find.text('Share'));
    await settle(tester);
    expect(files.origins.last, Rect.fromCenter(
      center: const Offset(400, 300),
      width: 2,
      height: 2,
    ));
  });

  testWidgets('Share through other files (the Mac toolbar\'s Share item) '
      'shares rather than exports, there and not to the save panel', (
    tester,
  ) async {
    late String aid;
    await tester.runAsync(() async {
      await me.ready();
      aid = (await me.api.createArtifact(
        PublishRequest(title: 'Plant log', files: {'index.html': '<p>hi</p>'}),
      )).artifact.id;
    });
    final mac = BundleFlows(
      exporter: me.exporter,
      installer: me.installer,
      updater: me.updater,
      files: files,
      settings: settings,
      open: opened.add,
      toFile: true,
    );
    final toolbar = FakeBundleFiles();
    await run(tester, (c) => mac.share(c, aid, via: toolbar));
    await tester.enterText(find.byKey(const ValueKey('export-name')), 'Alex');
    await tester.tap(find.text('Share'));
    await settle(tester);
    expect(toolbar.sent.single.$1, 'plant-log.clax');
    expect(files.sent, isEmpty);
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

  for (final developer in [false, true]) {
    testWidgets('an updater that throws ends the progress sheet with a '
        'failure; its raw text is under Details only with Developer tools '
        '(developer: $developer)', (tester) async {
      late Uint8List v2;
      await tester.runAsync(() async {
        await me.ready();
        final v1 = await makeBundle(lineage: lineageA);
        await me.installer.install(await me.installer.open(v1.bytes));
        v2 = (await makeBundle(
          lineage: lineageA,
          version: 2,
          files: {'index.html': '<p>v2</p>'},
        )).bytes;
      });
      flows = BundleFlows(
        exporter: me.exporter,
        installer: me.installer,
        updater: _ThrowingUpdater(me),
        files: files,
        settings: settings,
        open: opened.add,
      );
      await run(tester, (c) => flows.openBytes(c, v2), developer: developer);
      await tester.tap(find.text('Update'));
      await settle(tester);
      expect(find.text('The update could not finish.'), findsOneWidget);
      expect(find.textContaining('Bad state'), findsNothing);
      expect(find.text('Details'), developer ? findsOneWidget : findsNothing);
      if (developer) {
        await tester.tap(find.text('Details'));
        await settle(tester);
        expect(find.text('Bad state: broken'), findsOneWidget);
      }
    });
  }

  testWidgets('Cancel on the progress sheet stops the update and takes the '
      'sheet down', (tester) async {
    late Uint8List v2;
    await tester.runAsync(() async {
      await me.ready();
      final v1 = await makeBundle(lineage: lineageA);
      await me.installer.install(await me.installer.open(v1.bytes));
      v2 = (await makeBundle(lineage: lineageA, version: 2)).bytes;
    });
    final hanging = _HangingUpdater(me);
    flows = BundleFlows(
      exporter: me.exporter,
      installer: me.installer,
      updater: hanging,
      files: files,
      settings: settings,
      open: opened.add,
    );
    await run(tester, (c) => flows.openBytes(c, v2));
    await tester.tap(find.text('Update'));
    // The check's spinner never settles: pump through the sheet's entrance.
    for (var i = 0; i < 10; i++) {
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pump(const Duration(milliseconds: 50));
    }
    expect(find.text('Running its checks'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('cancel')));
    await settle(tester);
    expect(hanging.cancels, 1);
    expect(find.byType(BundleProgressSheet), findsNothing);
    expect(opened, isEmpty);
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
    await tester.tap(find.text('Keep mine'));
    await settle(tester);
    await tester.runAsync(() async {
      expect((await me.store.entry(aid))?.updatesStopped, isTrue);
    });
    expect(opened, isEmpty);
  });

  testWidgets('a damaged file: one sentence and Close', (tester) async {
    await run(tester, (c) => flows.openBytes(c, Uint8List.fromList([1, 2, 3])));
    expect(find.text('Couldn’t open this file'), findsOneWidget);
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

/// An [Updater] whose update throws.
class _ThrowingUpdater extends Updater {
  _ThrowingUpdater(Install me)
    : super(
        api: me.api,
        store: me.store,
        inspector: me.inspector,
        baseUrl: () => Uri.parse('http://127.0.0.1:9/'),
        snapshots: me.snapshots,
      );

  @override
  Future<UpdateOutcome> update(
    String aid,
    Bundle bundle, {
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
  }) async => throw StateError('broken');
}

/// An [Updater] whose check runs until it is cancelled, recording that.
class _HangingUpdater extends Updater {
  _HangingUpdater(Install me)
    : super(
        api: me.api,
        store: me.store,
        inspector: me.inspector,
        baseUrl: () => Uri.parse('http://127.0.0.1:9/'),
        snapshots: me.snapshots,
      );

  var cancels = 0;

  @override
  Future<UpdateOutcome> update(
    String aid,
    Bundle bundle, {
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
  }) async {
    onStep?.call(UpdateStep.tests);
    await cancelled;
    cancels++;
    return const UpdateCancelled();
  }
}
