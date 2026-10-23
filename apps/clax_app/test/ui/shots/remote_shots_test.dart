// The surfaces of an app's public copy, rendered in every form and theme
// into CLAX_SHOTS_DIR (skipped without it), and on the phone at 130% and
// 200% text: the connect sheet showing its code, the Share sheet with two
// people and link access, the On clax.dev card of a published app, the
// sheet after a first publish, and Get my apps.
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart' show Remote;
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/remote/remote_api.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/remote/connect_sheet.dart';
import 'package:clax_app/ui/remote/library_screen.dart';
import 'package:clax_app/ui/remote/published_sheet.dart';
import 'package:clax_app/ui/remote/remote_card.dart';
import 'package:clax_app/ui/remote/remote_flows.dart';
import 'package:clax_app/ui/remote/share_sheet.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../bundle/services_support.dart';
import '../../support/fake_clock.dart' show FakeClock;
import 'rig.dart';

String _h(int i) => 'sha256:${i.toRadixString(16).padLeft(64, '0')}';

/// Every form and theme at the usual text, and the phone at 130% and 200%.
void _shots(String surface, Future<void> Function(Shot s) body) {
  shots(surface, body);
  shots(surface, body, forms: const [phone], textScales: const [1.3, 2]);
}

/// Lets the fakes' futures run between frames, then draws.
Future<void> _settle(Shot s) async {
  for (var i = 0; i < 6; i++) {
    await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await s.tester.pump(const Duration(milliseconds: 100));
  }
  await s.settle();
}

/// Shows [sheet] (built with the page's context) over a page.
Future<void> _sheet(
  Shot s,
  void Function(BuildContext context) sheet,
) async {
  final page = GlobalKey();
  await s.tester.pumpWidget(
    s.app(
      Scaffold(
        key: page,
        appBar: AppBar(title: const Text('Plant log')),
        body: const Center(child: Text('A page')),
      ),
    ),
  );
  sheet(page.currentContext!);
  await _settle(s);
}

void main() {
  _shots('remote-connect', (s) async {
    final remote = FakeRemoteApi()..signInsWait = true;
    final clock = FakeClock();
    await _sheet(s, (context) {
      Future<Remote?> _ = showConnectSheet(
        context,
        remote,
        clock: clock,
        device: 'Pixel 9',
        openAuth: (_) async => true,
        closeAuth: () async {},
      );
    });
    await s.tester.runAsync(() => clock.advance(Duration.zero));
    await _settle(s);
    await s.take('remote-connect');
  });

  _shots('remote-share', (s) async {
    final remote = FakeRemoteApi();
    final r = remote.connected('https://clax.dev');
    final copy = remote.addCopy(
      r.id,
      contentSha256: _h(1),
      title: 'Plant log',
      access: 'owner',
      general: 'link',
      comments: true,
    );
    await s.tester.runAsync(() async {
      await remote.share(r.id, copy.raid, ShareLogin('junp'), 'editor');
      await remote.share(
        r.id,
        copy.raid,
        ShareLogin('alexandra-longname'),
        'commenter',
      );
    });
    await _sheet(s, (context) {
      Future<void> _ = showShareSheet(
        context,
        remote,
        rid: r.id,
        raid: copy.raid,
        title: 'Plant log',
        host: 'clax.dev',
        copyText: (_) async {},
        shareLink: (_, _, _) async {},
      );
    });
    await s.take('remote-share');
    // Its end: the people, General access and the link's.
    await s.tester.drag(
      find.text('Share Plant log'),
      const Offset(0, -2000),
      warnIfMissed: false,
    );
    await _settle(s);
    await s.take('remote-share-end');
  });

  _shots('remote-card', (s) async {
    final tmp = Directory.systemTemp.createTempSync('remote-shots');
    addTearDown(() => tmp.deleteSync(recursive: true));
    late RemoteFlows flows;
    late String aid;
    late String live;
    late LineageEntry entry;
    await s.tester.runAsync(() async {
      final me = Install(tmp);
      await me.ready();
      final remote = FakeRemoteApi()..connected('https://clax.dev');
      final settings = SettingsStore(File('${tmp.path}/settings.json'));
      final publisher = Publisher(
        remote: remote,
        exporter: me.exporter,
        store: me.store,
        clock: FakeClock(),
      );
      flows = RemoteFlows(
        remote: remote,
        publisher: publisher,
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
      await publisher.publish(aid, version: 1);
      entry = (await me.store.entry(aid))!;
    });
    await s.tester.pumpWidget(
      s.app(
        Scaffold(
          appBar: AppBar(title: const Text('Plant log')),
          body: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: RemoteCard(
              flows: flows,
              aid: aid,
              entry: entry,
              liveContentSha256: live,
              modified: false,
              onChanged: () {},
            ),
          ),
        ),
      ),
    );
    await _settle(s);
    await s.take('remote-card');
  });

  _shots('remote-published', (s) async {
    await _sheet(s, (context) {
      Future<PublishedAction?> _ = showBundleSheet<PublishedAction>(
        context,
        const PublishedSheet(
          title: 'Plant log',
          host: 'clax.dev',
          version: 1,
          url: 'https://clax.dev/a/r0000000000r',
          seed: 'a0000000000a',
        ),
      );
    });
    await s.take('remote-published');
  });

  _shots('remote-library', (s) async {
    final remote = FakeRemoteApi();
    final r = remote.connected('https://clax.dev');
    final mine = remote.addCopy(
      r.id,
      contentSha256: _h(1),
      title: 'Plant log',
      access: 'owner',
    );
    final shared = remote.addCopy(
      r.id,
      contentSha256: _h(2),
      title: 'Run tracker with a long name for the weekend races',
      access: 'commenter',
    );
    for (final (c, bv) in [(mine, 4), (shared, 2)]) {
      c.versions.single.bundle = FakeBundle(
        bytes: Uint8List(0),
        lineage: 'l_${c.raid}',
        bundleVersion: bv,
        contentSha256: c.versions.single.contentSha256,
      );
    }
    await s.tester.pumpWidget(
      s.app(
        LibraryScreen(
          remote: remote,
          rid: r.id,
          pull: (_) async => const PullInstalled('a_new'),
        ),
      ),
    );
    await _settle(s);
    await s.take('remote-library');
  });
}
