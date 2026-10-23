import 'dart:io';

import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/remote/remote_card.dart';
import 'package:clax_app/ui/remote/remote_flows.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_clock.dart' show FakeClock;
import '../support/settle.dart';

void main() {
  late Directory tmp;
  late Install me;
  late FakeRemoteApi remote;
  late RemoteFlows flows;
  late String aid;
  late String live;

  Future<void> setUpApp(WidgetTester tester) => tester.runAsync(() async {
    tmp = Directory.systemTemp.createTempSync('remote-card');
    me = Install(tmp);
    await me.ready();
    remote = FakeRemoteApi()..connected('https://clax.example');
    final settings = SettingsStore(File('${tmp.path}/settings.json'));
    flows = RemoteFlows(
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
    );
    final made = await me.api.createArtifact(
      PublishRequest(
        title: 'Plant log',
        entries: {'index.html': FileEntry.text('<p>v1</p>')},
      ),
    );
    aid = made.artifact.id;
    final v = (await me.api.getArtifact(aid)).versions.single;
    live =
        v.contentSha256 ??
        contentHash({
          for (final MapEntry(:key, :value) in v.files.entries)
            key: (sha256: value.sha256 ?? '', size: value.size),
        });
    remote.localVersion(aid, 1, contentSha256: live, title: 'Plant log');
  });

  Widget card(LineageEntry? entry, {VoidCallback? onChanged}) => MaterialApp(
    theme: claxTheme(Brightness.light, defaultTargetPlatform),
    home: Scaffold(
      body: RemoteCard(
        flows: flows,
        aid: aid,
        entry: entry,
        liveContentSha256: live,
        modified: false,
        onChanged: onChanged ?? () {},
      ),
    ),
  );

  testWidgets('an app with no link offers Publish to clax.dev…', (
    tester,
  ) async {
    await setUpApp(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    await tester.pumpWidget(card(null));
    expect(find.text('Publish to clax.dev…'), findsOneWidget);
    expect(find.byKey(const ValueKey('remote-card')), findsNothing);
  });

  testWidgets('a published app shows its copy, Live, and its author '
      'controls; New versions sets the link', (tester) async {
    await setUpApp(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    late LineageEntry entry;
    await tester.runAsync(() async {
      final out = await flows.publisher.publish(aid, version: 1);
      expect(out, isA<Published>());
      entry = (await me.store.entry(aid))!;
    });
    var changed = 0;
    await tester.pumpWidget(card(entry, onChanged: () => changed++));
    await settle(tester, find.byKey(const ValueKey('remote-pill')));

    expect(find.text('On clax.example'), findsOneWidget);
    expect(find.text(entry.link!.url), findsOneWidget);
    expect(
      tester.widget<Text>(find.byKey(const ValueKey('remote-pill'))).data,
      'Live',
    );
    // The cloud is an icon screen readers skip, not a glyph in the words.
    expect(
      find.descendant(
        of: find.byType(ExcludeSemantics),
        matching: find.byKey(const ValueKey('remote-pill-icon')),
      ),
      findsOneWidget,
    );
    expect(find.text('Share…'), findsOneWidget);
    expect(find.text('Open on clax.example'), findsOneWidget);
    expect(find.text('Unpublish…'), findsOneWidget);

    await tester.tap(find.text('Automatically'));
    await settleUntil(tester, () => changed == 1);
    await tester.runAsync(() async {
      expect((await me.store.entry(aid))!.link!.autoPublish, isTrue);
    });
    expect(changed, 1);
  });

  testWidgets('a copy no longer there says so and offers no Unpublish', (
    tester,
  ) async {
    await setUpApp(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    late LineageEntry entry;
    await tester.runAsync(() async {
      await flows.publisher.publish(aid, version: 1);
      entry = (await me.store.entry(aid))!;
    });
    remote.copies.clear();
    await tester.pumpWidget(card(entry));
    await settle(tester, find.byKey(const ValueKey('remote-pill')));
    expect(
      tester.widget<Text>(find.byKey(const ValueKey('remote-pill'))).data,
      'No longer on clax.example',
    );
    expect(find.text('Unpublish…'), findsNothing);
    expect(find.text('Automatically'), findsNothing);
    expect(find.text('Open on clax.example'), findsOneWidget);
  });

  testWidgets('a copy got from someone shows no author controls', (
    tester,
  ) async {
    await setUpApp(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    late LineageEntry entry;
    await tester.runAsync(() async {
      await flows.publisher.publish(aid, version: 1);
      final e = (await me.store.entry(aid))!;
      entry = e.copyWith(
        link: () => e.link!.copyWith(role: AppLink.recipient),
      );
    });
    await tester.pumpWidget(card(entry));
    await settle(tester, find.byKey(const ValueKey('remote-pill')));
    expect(find.text('On clax.example'), findsOneWidget);
    expect(find.text('Open on clax.example'), findsOneWidget);
    expect(find.text('Share…'), findsNothing);
    expect(find.text('Unpublish…'), findsNothing);
    expect(find.text('Automatically'), findsNothing);
  });

  testWidgets('the inspector\'s Versions tab publishes a version of a '
      'published app', (tester) async {
    await setUpApp(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    await tester.runAsync(() async {
      await flows.publisher.publish(aid, version: 1);
      await me.api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>v2</p>'}),
      );
      final v2 = (await me.api.getArtifact(aid)).versions.last;
      remote.localVersion(aid, 2, contentSha256: v2.contentSha256!);
    });
    tester.view.physicalSize = const Size(800, 1720);
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: AppInspector(
          api: me.api,
          aid: aid,
          lineage: me.store.entry,
          remote: flows,
          onOpenVersion: (_, _) {},
        ),
      ),
    );
    await settle(tester, find.byKey(const ValueKey('remote-card')));
    await tester.tap(find.widgetWithText(Tab, 'Versions'));
    final publish = find.byKey(const ValueKey('publish-2'));
    await settle(tester, publish);
    expect(find.byKey(const ValueKey('mark-1')), findsOneWidget);
    expect(tester.widget<TextButton>(publish).onPressed, isNotNull);
    await tester.tap(publish);
    await settle(tester, find.text('Plant log is on clax.example'));
    expect(remote.copies.values.single.versions, hasLength(2));
    expect(find.text('Plant log is on clax.example'), findsOneWidget);
  });
}
