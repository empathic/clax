import 'dart:io';

import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/remote/publish_sheet.dart';
import 'package:clax_app/ui/remote/published_sheet.dart';
import 'package:clax_app/ui/remote/remote_flows.dart';
import 'package:clax_app/ui/tile_label.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_clock.dart' show FakeClock;
import '../support/fake_voice.dart';
import '../support/settle.dart';
import 'bundle_flows_test.dart' show FakeBundleFiles;

const _sheet = PublishSheet(
  title: 'Plant log',
  host: 'clax.dev',
  login: 'alex',
  version: 3,
  fileCount: 2,
  fileBytes: 1200,
  dataAllowed: true,
);

void main() {
  testWidgets('the published sheet says who can open it in words alone, '
      'with no lock glyph the system font may lack', (tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(
          body: PublishedSheet(
            title: 'Plant log',
            host: 'clax.dev',
            version: 1,
            url: 'https://clax.dev/a/r0000000000r',
          ),
        ),
      ),
    );
    expect(find.text('v1 · only you so far'), findsOneWidget);
  });

  /// Pumps a button that shows [sheet] and keeps what it pops in
  /// [popped].
  Future<List<PublishChoice?>> pumpSheet(
    WidgetTester tester, [
    PublishSheet sheet = _sheet,
  ]) async {
    final popped = <PublishChoice?>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async =>
                popped.add(await showBundleSheet<PublishChoice>(context, sheet)),
            child: const Text('go'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('go'));
    await tester.pumpAndSettle();
    return popped;
  }

  testWidgets('the publish sheet says who can open it, what goes up and '
      'what stays here', (tester) async {
    final popped = await pumpSheet(tester);
    for (final line in [
      'Publish Plant log',
      'to clax.dev as @alex',
      'Who can open it',
      'Only you, until you share it',
      'Restricted',
      'Include my data',
      'Publish new versions',
      'Ask me',
      'Automatically',
      'Comments',
      'Off: nobody can comment, whatever their access',
      'Goes up: v3 · 2 files · 1 KB',
      'Stays here: your data, earlier versions, threads, API key',
    ]) {
      expect(find.text(line), findsOneWidget, reason: line);
    }
    // The sheet scrolls; Publish is under its fold on the test screen.
    await tester.ensureVisible(find.byKey(const ValueKey('publish')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('publish')));
    await tester.pumpAndSettle();
    expect(popped.single?.includeData, isFalse);
    expect(popped.single?.autoPublish, isFalse);
  });

  testWidgets('Publish fork asks for the fork’s name, prefilled, and waits '
      'for one', (tester) async {
    final popped = await pumpSheet(
      tester,
      const PublishSheet(
        title: 'Plant log',
        host: 'clax.dev',
        login: 'alex',
        version: 3,
        fileCount: 2,
        fileBytes: 1200,
        dataAllowed: true,
        forkName: 'Plant log (yours)',
      ),
    );
    expect(find.text('Publish fork'), findsOneWidget);
    expect(find.text('Restricted'), findsOneWidget);
    // The lock is an icon, not a glyph the system font may lack.
    expect(find.byIcon(Icons.lock_outline_rounded), findsOneWidget);
    expect(
      find.text('Off: nobody can comment, whatever their access'),
      findsOneWidget,
    );
    final field = find.byKey(const ValueKey('fork-name'));
    expect(
      find.descendant(of: field, matching: find.text('Plant log (yours)')),
      findsOneWidget,
    );
    await tester.enterText(
      find.descendant(of: field, matching: find.byType(EditableText)),
      ' ',
    );
    await tester.pumpAndSettle();
    final publish = find.byKey(const ValueKey('publish'));
    await tester.ensureVisible(publish);
    await tester.pumpAndSettle();
    await tester.tap(publish);
    await tester.pumpAndSettle();
    expect(popped, isEmpty, reason: 'no name, no publish');
    await tester.enterText(
      find.descendant(of: field, matching: find.byType(EditableText)),
      'My plants',
    );
    await tester.pumpAndSettle();
    await tester.ensureVisible(publish);
    await tester.pumpAndSettle();
    await tester.tap(publish);
    await tester.pumpAndSettle();
    expect(popped.single?.name, 'My plants');
  });

  testWidgets('Include my data and Automatically are the person’s choice '
      'for this publish', (tester) async {
    final popped = await pumpSheet(tester);
    await tester.ensureVisible(find.text('Include my data'));
    await tester.tap(find.text('Include my data'));
    await tester.ensureVisible(find.text('Automatically'));
    await tester.tap(find.text('Automatically'));
    await tester.pump();
    // With the data going up, the sheet no longer says it stays here.
    expect(find.text('Goes up: v3 · 2 files · 1 KB · your data'), findsOneWidget);
    expect(
      find.text('Stays here: earlier versions, threads, API key'),
      findsOneWidget,
    );
    // The sheet scrolls; Publish is under its fold on the test screen.
    await tester.ensureVisible(find.byKey(const ValueKey('publish')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('publish')));
    await tester.pumpAndSettle();
    expect(popped.single?.includeData, isTrue);
    expect(popped.single?.autoPublish, isTrue);
  });

  testWidgets('menu_has_publish_above_share', (tester) async {
    late Directory tmp;
    late Install me;
    await tester.runAsync(() async {
      tmp = Directory.systemTemp.createTempSync('remote-menu');
      me = Install(tmp);
      await me.ready();
    });
    addTearDown(() => tmp.deleteSync(recursive: true));
    me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    final remote = FakeRemoteApi()..connected('https://clax.dev');
    final settings = SettingsStore(File('${tmp.path}/settings.json'));
    await tester.pumpWidget(
      MaterialApp(
        home: Launcher(
          api: me.api,
          open: (_) {},
          make: (r, c, s) => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
          bundles: BundleFlows(
            exporter: me.exporter,
            installer: me.installer,
            updater: me.updater,
            files: FakeBundleFiles(),
            settings: settings,
            open: (_) {},
          ),
          remote: RemoteFlows(
            remote: remote,
            publisher: Publisher(
              remote: remote,
              exporter: me.exporter,
              store: me.store,
              clock: FakeClock(),
            ),
            puller: Puller(
              remote: remote,
              installer: me.installer,
              store: me.store,
              installId: () => ensureInstallId(settings),
            ),
            exporter: me.exporter,
            store: me.store,
            open: (_) {},
          ),
          lineage: me.store.entry,
        ),
      ),
    );
    final tile = find.byWidgetPredicate(
      (w) => w is TileLabel && w.text == 'Plant log',
    );
    await settle(tester, tile);
    await tester.longPress(tile);
    await tester.pumpAndSettle();
    final publish = find.text('Publish to clax.dev…');
    final share = find.text('Share app');
    expect(publish, findsOneWidget);
    expect(share, findsOneWidget);
    expect(
      tester.getTopLeft(publish).dy,
      lessThan(tester.getTopLeft(share).dy),
    );
  });
}
