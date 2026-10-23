import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/author_trust.dart';
import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/merge.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/ui/kit/kit.dart' show ClaxCheckbox;
import 'package:clax_app/ui/remote/published_sheet.dart';
import 'package:clax_app/ui/remote/remote_flows.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_clock.dart' show FakeClock;
import '../support/settle.dart';

/// The core's answer for a remote it cannot reach.
const _offline = ClaxApiError(
  status: 502,
  code: 'remote_unreachable',
  message: 'https://clax.example could not be reached',
);

void main() {
  late Directory tmp;
  late Install me;
  late FakeRemoteApi remote;
  late Remote r;
  late FakeClock clock;
  late RemoteFlows flows;

  Future<void> setUpFlows(WidgetTester tester) => tester.runAsync(() async {
    tmp = Directory.systemTemp.createTempSync('remote-flows');
    me = Install(tmp);
    await me.ready();
    remote = FakeRemoteApi();
    r = remote.connected('https://clax.example');
    clock = FakeClock();
    final settings = SettingsStore(File('${tmp.path}/settings.json'));
    final publisher = Publisher(
      remote: remote,
      exporter: me.exporter,
      store: me.store,
      clock: clock,
    );
    flows = RemoteFlows(
      remote: remote,
      publisher: publisher,
      merger: Merger(
        api: me.api,
        store: me.store,
        updater: me.updater,
        publisher: publisher,
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
    me.updater.relink = remote.link;
  });

  /// Whether the flow the last tap on [button] started has ended.
  var flowDone = false;

  /// A button that runs [run] with its context.
  Widget button(Future<void> Function(BuildContext context) run) =>
      MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () {
                flowDone = false;
                run(context).whenComplete(() => flowDone = true);
              },
              child: const Text('go'),
            ),
          ),
        ),
      );

  Future<void> tapVisible(WidgetTester tester, Finder f) async {
    await tester.ensureVisible(f);
    await tester.pump(const Duration(milliseconds: 300));
    await tester.tap(f);
  }

  testWidgets('offline, the publish sheet closes and the publish still '
      'lands later, in the background', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final aid = me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    final v = (await tester.runAsync(() => me.api.getArtifact(aid)))!;
    remote.localVersion(
      aid,
      1,
      contentSha256: v.versions.single.contentSha256!,
      title: 'Plant log',
    );
    remote.failNext('publish', _offline);
    await tester.pumpWidget(button((c) => flows.publish(c, aid)));
    await tester.tap(find.text('go'));
    await settle(tester, _publishButton);
    await tapVisible(tester, _publishButton);
    await settle(tester, find.text(RemoteFlows.waitingStep));

    expect(find.text(RemoteFlows.waitingStep), findsOneWidget);
    expect(flows.publisher.waiting.value, {aid});
    await tapVisible(tester, find.byKey(const ValueKey('close')));
    await settleUntil(tester, () => _gone(find.text('Publishing Plant log…')));
    expect(find.text('Publishing Plant log…'), findsNothing);
    expect(remote.copies, isEmpty);

    // The retry is due on the clock; it lands with no sheet up, once the
    // upload's file writes finish.
    await tester.runAsync(() => clock.advance(const Duration(seconds: 2)));
    await settleUntil(
      tester,
      () => flows.publisher.waiting.value.isEmpty && remote.copies.isNotEmpty,
    );
    final copy = remote.copies.values.single;
    expect(copy.versions.single.bundle, isNotNull);
    expect(flows.publisher.waiting.value, isEmpty);
    expect(find.text('Plant log is on clax.example'), findsNothing);
  });

  testWidgets('after Disconnect, Publish signs in again through connect and '
      'publishes the next version on the same copy', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final signIns = <String>[];
    flows = RemoteFlows(
      remote: remote,
      publisher: flows.publisher,
      puller: flows.puller,
      exporter: me.exporter,
      store: me.store,
      open: (_) {},
      connect: (context) async {
        signIns.add('connect');
        return remote.connected(r.origin);
      },
    );
    final aid = me.api.seedArtifact('Plant log', {'index.html': '<p>1</p>'});
    await tester.runAsync(() async {
      await me.api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>2</p>'}),
      );
      for (final v in (await me.api.getArtifact(aid)).versions) {
        remote.localVersion(
          aid,
          v.n,
          contentSha256: v.contentSha256!,
          title: 'Plant log',
        );
      }
    });
    await tester.pumpWidget(
      button((c) => flows.publish(c, aid, version: signIns.isEmpty ? 1 : 2)),
    );
    await tester.tap(find.text('go'));
    await settle(tester, _publishButton);
    await tapVisible(tester, _publishButton);
    await settle(tester, find.byType(PublishedSheet));
    await tapVisible(tester, find.text('Done'));
    await settleUntil(tester, () => _gone(find.byType(PublishedSheet)));
    final copy = remote.copies.values.single;
    expect(copy.versions, hasLength(1));
    expect(signIns, isEmpty);

    await tester.runAsync(() => remote.removeRemote(r.id));
    await tester.tap(find.text('go'));
    await settle(tester, find.byType(PublishedSheet));
    expect(signIns, ['connect']);
    expect(find.text('Plant log is on clax.example'), findsOneWidget);
    expect(remote.copies, hasLength(1));
    expect(copy.versions, hasLength(2));
    expect(copy.versions[1].bundle, isNotNull);
  });

  testWidgets('a sign-in at another origin is not taken for a linked app', (
    tester,
  ) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final aid = me.api.seedArtifact('Plant log', {'index.html': '<p>1</p>'});
    await tester.runAsync(() async {
      final v = await me.api.getArtifact(aid);
      remote.localVersion(
        aid,
        1,
        contentSha256: v.versions.single.contentSha256!,
        title: 'Plant log',
      );
      await flows.publisher.publish(aid, version: 1);
      await remote.removeRemote(r.id);
    });
    flows = RemoteFlows(
      remote: remote,
      publisher: flows.publisher,
      puller: flows.puller,
      exporter: me.exporter,
      store: me.store,
      open: (_) {},
      connect: (context) async => remote.connected('https://other.example'),
    );
    await tester.pumpWidget(button((c) => flows.publish(c, aid)));
    await tester.tap(find.text('go'));
    await settleUntil(tester, () => flowDone);
    expect(remote.calls.where((c) => c == 'publish'), hasLength(1));
    expect(remote.copies.values.single.versions, hasLength(1));
    // Not taken as this app's remote, so no publish was tried through it.
    expect(find.textContaining('not signed in'), findsNothing);
    expect(find.textContaining('Publishing'), findsNothing);
  });

  testWidgets('Get my apps asks with the install sheet, who it is from '
      'included, before it installs', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final w = (await tester.runAsync(
      () => makeBundle(lineage: lineageA, version: 2),
    ))!;
    final c = remote.addCopy(
      r.id,
      contentSha256: w.manifest.contentSha256,
      title: 'Plant log',
      access: 'commenter',
    );
    c.versions.single.bundle = FakeBundle(
      bytes: w.bytes,
      lineage: lineageA,
      bundleVersion: 2,
      contentSha256: w.manifest.contentSha256,
    );
    await tester.pumpWidget(button(flows.getMyApps));
    await tester.tap(find.text('go'));
    final pick = find.byKey(ValueKey('pick-${c.raid}'));
    await settle(tester, pick);

    Future<void> getIt() async {
      await tester.tap(pick);
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('library-get')));
      await settle(tester, _installButton);
    }

    await getIt();
    expect(find.byKey(const ValueKey('install-from')), findsOneWidget);
    expect(find.textContaining('owner'), findsWidgets);
    await tapVisible(tester, find.text('Cancel'));
    // Done getting: its box takes a tick again.
    await settleUntil(
      tester,
      () =>
          _gone(_installButton) &&
          tester.widget<ClaxCheckbox>(pick).onChanged != null,
    );
    expect(await tester.runAsync(me.api.listArtifacts), isEmpty);

    await getIt();
    await tapVisible(tester, _installButton);
    await settle(tester, find.text('Got it'));
    expect(await tester.runAsync(me.api.listArtifacts), hasLength(1));
    expect(find.text('Got it'), findsOneWidget);
  });

  /// Gets copy [c] through Get my apps, as its install sheet asks; answers
  /// the app made here.
  Future<String> getFromLibrary(WidgetTester tester, FakePublicCopy c) async {
    await tester.pumpWidget(button(flows.getMyApps));
    await tester.tap(find.text('go'));
    await settle(tester, find.byKey(ValueKey('pick-${c.raid}')));
    await tester.tap(find.byKey(ValueKey('pick-${c.raid}')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('library-get')));
    await settle(tester, _installButton);
    await tapVisible(tester, _installButton);
    await settle(tester, find.text('Got it'));
    return (await tester.runAsync(me.api.listArtifacts))!.single.id;
  }

  /// A copy of a bundle signed elsewhere, at [access], owned by [owner].
  Future<(FakePublicCopy, String)> libraryCopy(
    WidgetTester tester, {
    required String access,
    required PublishedBy owner,
  }) async {
    final w = (await tester.runAsync(
      () => makeBundle(lineage: lineageA, version: 2),
    ))!;
    final c = remote.addCopy(
      r.id,
      contentSha256: w.manifest.contentSha256,
      title: 'Plant log',
      access: access,
      owner: owner,
    );
    c.versions.single.bundle = FakeBundle(
      bytes: w.bytes,
      lineage: lineageA,
      bundleVersion: 2,
      contentSha256: w.manifest.contentSha256,
    );
    return (c, w.manifest.author.key);
  }

  AuthorTrust trustHere() => AuthorTrust(
    lineages: me.store,
    settings: SettingsStore(File('${tmp.path}/trust.json')),
    ownKey: () async => (await me.key.existing())?.key,
    ownAccount: (origin, by) async =>
        isOwnAccount(await remote.remotes(), origin, by),
  );

  testWidgets('a copy got from the library as its editor publishes its next '
      'version to that same copy, and its signer is still asked about', (
    tester,
  ) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (c, signer) = await libraryCopy(
      tester,
      access: 'editor',
      owner: const PublishedBy(name: 'Sam', login: 'sam', publicId: 'p_sam'),
    );
    final aid = await getFromLibrary(tester, c);
    final got = (await tester.runAsync(() => me.store.entry(aid)))!;
    expect(got.role, LineageRole.installed);
    expect(got.link!.isAuthor, isTrue);
    expect(got.publishesAsFork, isFalse);
    expect(flows.publishLabelOf(got), flows.publishLabel);

    // The person's own change, published as the copy's next version.
    await tester.runAsync(
      () => me.api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>mine</p>'}),
      ),
    );
    final v2 = (await tester.runAsync(() => me.api.getArtifact(aid)))!;
    remote.localVersion(
      aid,
      2,
      contentSha256: v2.versions.last.contentSha256!,
      title: 'Plant log',
    );
    // A fresh screen: the install's sheet is done with.
    await tester.pumpWidget(const SizedBox());
    await tester.pumpWidget(button((c) => flows.publish(c, aid, version: 2)));
    await tester.tap(find.text('go'));
    await settle(tester, find.byType(PublishedSheet));
    expect(remote.copies.keys, [c.raid], reason: 'no new copy');
    expect(c.versions, hasLength(2));
    final e = (await tester.runAsync(() => me.store.entry(aid)))!;
    expect(e.link!.remoteArtifactId, c.raid);
    expect(e.link!.published.last, 2);

    // The publish made the entry the person's own; Sam's signed code is
    // still in it, so Claude asks before reading it.
    expect(e.role, LineageRole.author);
    expect(e.liveFrom?.key, signer);
    expect(await trustHere().ensure(null, aid), isFalse);
  });

  testWidgets('an editor who takes an update publishes the next version on '
      'the same copy, with nothing moved', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    const sam = PublishedBy(name: 'Sam', login: 'sam', publicId: 'p_sam');
    final (c, _) = await libraryCopy(tester, access: 'editor', owner: sam);
    // Got from the library (as Get my apps gets it), in the real zone, as
    // every store write of this test is.
    final got = await tester.runAsync(() async {
      final app = (await remote.apps(r.id)).single;
      return flows.puller.pullFromLibrary(r.id, app);
    });
    final aid = (got as PullInstalled).aid;
    expect(remote.links[aid]?.remoteN, 1);

    // Sam publishes v2 there, with its bundle; this device takes it.
    final w = (await tester.runAsync(
      () => makeBundle(
        lineage: lineageA,
        version: 3,
        files: {'index.html': '<p>Sam’s v2</p>'},
      ),
    ))!;
    remote.publishThere(
      c.raid,
      contentSha256: w.manifest.contentSha256,
      by: sam,
      bundle: FakeBundle(
        bytes: w.bytes,
        lineage: lineageA,
        bundleVersion: 3,
        contentSha256: w.manifest.contentSha256,
      ),
    );
    final out = await tester.runAsync(() async {
      final head = await remote.head(r.id, c.raid);
      return me.updater.update(
        aid,
        await BundleReader.read(w.bytes),
        origin: BundleOrigin(
          remoteOrigin: r.origin,
          remoteArtifactId: c.raid,
          remoteVersion: 2,
          head: head,
        ),
      );
    });
    expect(out, isA<UpdateLanded>());
    expect((out as UpdateLanded).notLinked, isNull);
    // Both bases moved to public v2: here and in the core.
    expect(remote.links[aid]?.remoteN, 2);
    final e = (await tester.runAsync(() => me.store.entry(aid)))!;
    expect(e.link!.baseRemoteVersion, 2);

    // The person's change on top, published: the same copy's v3.
    await tester.runAsync(() async {
      final n = (await me.api.getArtifact(aid)).artifact.currentVersion;
      await me.api.publish(
        aid,
        PublishRequest(ifVersion: n, files: {'index.html': '<p>mine</p>'}),
      );
      final d = await me.api.getArtifact(aid);
      remote.localVersion(
        aid,
        d.artifact.currentVersion,
        contentSha256: d.versions.last.contentSha256!,
        title: 'Plant log',
      );
    });
    final n = (await tester.runAsync(() => me.api.getArtifact(aid)))!
        .artifact
        .currentVersion;
    await tester.pumpWidget(const SizedBox());
    await tester.pumpWidget(button((c) => flows.publish(c, aid, version: n)));
    await tester.tap(find.text('go'));
    await settle(tester, find.byType(PublishedSheet));
    expect(find.text('clax.example moved on'), findsNothing);
    expect(remote.copies.keys, [c.raid]);
    expect(c.versions, hasLength(3));
  });

  testWidgets('a copy got from the person’s own account asks nothing', (
    tester,
  ) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    remote.remotesById[r.id] = Remote(
      id: r.id,
      origin: r.origin,
      name: r.name,
      status: 'connected',
      personName: 'Me',
      personPublicId: 'p_me',
    );
    final (c, _) = await libraryCopy(
      tester,
      access: 'owner',
      owner: const PublishedBy(name: 'Me', login: 'me', publicId: 'p_me'),
    );
    final aid = await getFromLibrary(tester, c);
    expect(await trustHere().ensure(null, aid), isTrue);
    // Someone else named Me is not the person.
    await tester.runAsync(
      () => me.store.update(
        aid,
        (e) async => e!.copyWith(
          liveFrom: () => VersionSource(
            key: e.key,
            who: '@me',
            origin: r.origin,
            account: const PublishedBy(
              name: 'Me',
              login: 'me',
              publicId: 'p_other',
            ),
          ),
        ),
      ),
    );
    expect(await trustHere().ensure(null, aid), isFalse);
  });

  /// App [aid] made here and published once (public v1).
  Future<(String, FakePublicCopy)> published(WidgetTester tester) async {
    final aid = me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
    final v = (await tester.runAsync(() => me.api.getArtifact(aid)))!;
    remote.localVersion(
      aid,
      1,
      contentSha256: v.versions.single.contentSha256!,
      title: 'Plant log',
    );
    final out = await tester.runAsync(
      () => flows.publisher.publish(aid, version: 1),
    );
    expect(out, isA<Published>());
    return (aid, remote.copies.values.single);
  }

  testWidgets('tell_owner_prefills_and_needs_comments_on: Tell @owner is '
      'offered only while the copy takes comments and the account may '
      'comment', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (aid, copy) = await published(tester);

    // Comments off (the first publish's default): no Tell @owner.
    copy.comments = false;
    expect(
      await tester.runAsync(() => flows.tellOwner(aid, 3, 'its tests failed')),
      isNull,
    );

    // On, for an author: offered, prefilled with the version and reason,
    // and posting it comments on the copy.
    copy.comments = true;
    final tell = (await tester.runAsync(
      () => flows.tellOwner(aid, 3, 'its tests failed'),
    ))!;
    expect(tell.owner, '@me');
    expect(tell.draft, 'v3 didn’t pass its checks on my data: its tests failed.');
    expect(copy.posted, isEmpty);
    expect(await tester.runAsync(() => tell.post('It broke here.')), isNull);
    expect(copy.posted, ['It broke here.']);

    // A recipient who may only view: none; one who may comment: offered.
    await tester.runAsync(
      () => me.store.update(
        aid,
        (e) async => e!.copyWith(
          link: () => e.link!.copyWith(role: AppLink.recipient),
        ),
      ),
    );
    copy.access = 'viewer';
    expect(
      await tester.runAsync(() => flows.tellOwner(aid, 3, 'its tests failed')),
      isNull,
    );
    copy.access = 'commenter';
    expect(
      await tester.runAsync(() => flows.tellOwner(aid, 3, 'its tests failed')),
      isNotNull,
    );

    // Comments turned off after the offer: the post says so.
    copy.comments = false;
    expect(
      await tester.runAsync(() => tell.post('x')),
      'Comments are off on this app now.',
    );
  });

  testWidgets('Unpublish with the remote out of reach says the app is still '
      'on it; a hosted Clax too old says it needs an update', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (aid, _) = await published(tester);
    await tester.pumpWidget(button((c) => flows.unpublish(c, aid)));

    Future<void> unpublish(Finder shows) async {
      await tester.tap(find.text('go'));
      await settle(tester, find.text('Unpublish'));
      await tapVisible(tester, find.text('Unpublish'));
      await settle(tester, shows);
    }

    final unreachable = find.text(
      'Couldn’t reach clax.example. Plant log is still on it.',
    );
    remote.failNext('unpublish', _offline);
    await unpublish(unreachable);
    expect(unreachable, findsOneWidget);
    await tapVisible(tester, find.text('Close'));
    await settleUntil(tester, () => _gone(unreachable));

    remote.tooOld.add('https://clax.example');
    final tooOld = find.text(
      'clax.example needs an update before the Clax app can unpublish '
      'apps.',
    );
    await unpublish(tooOld);
    expect(tooOld, findsOneWidget);
    expect(remote.copies, hasLength(1));
  });

  testWidgets('Publish as a new app offline can be closed, says it waits for '
      'a connection, and lands later in the background', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (aid, old) = await published(tester);
    remote.failNext('publish', _offline);
    await tester.pumpWidget(button((c) => flows.publishAsNewApp(c, aid)));
    await tester.tap(find.text('go'));
    await settle(tester, find.text(RemoteFlows.waitingStep));
    expect(find.text(RemoteFlows.waitingStep), findsOneWidget);
    await tapVisible(tester, find.byKey(const ValueKey('close')));
    await settleUntil(
      tester,
      () => _gone(find.text('Publishing Plant log as a new app…')),
    );
    expect(find.text('Publishing Plant log as a new app…'), findsNothing);

    await tester.runAsync(() => clock.advance(const Duration(seconds: 2)));
    await settleUntil(
      tester,
      () =>
          flows.publisher.waiting.value.isEmpty && remote.copies.length == 2,
    );
    expect(remote.copies, hasLength(2));
    final link = (await tester.runAsync(() => me.store.entry(aid)))!.link!;
    expect(link.remoteArtifactId, isNot(old.raid));
    expect(find.textContaining('is a new app'), findsNothing);
  });

  testWidgets('Unpublish stops a publish still waiting for a connection, so '
      'it does not make the app public again', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (aid, _) = await published(tester);
    remote.failNext('publish', _offline);
    late final Future<PublishOutcome> waiting;
    // Once it waits for a connection.
    await tester.runAsync(() async {
      waiting = flows.publisher.publish(aid, version: 1);
      final waits = Completer<void>();
      void check() {
        if (flows.publisher.waiting.value.contains(aid) && !waits.isCompleted) {
          waits.complete();
        }
      }

      flows.publisher.waiting.addListener(check);
      check();
      await waits.future.timeout(const Duration(minutes: 1));
      flows.publisher.waiting.removeListener(check);
    });
    expect(flows.publisher.waiting.value, {aid});

    await tester.pumpWidget(button((c) => flows.unpublish(c, aid)));
    await tester.tap(find.text('go'));
    await settle(tester, find.text('Unpublish'));
    await tapVisible(tester, find.text('Unpublish'));
    await settleUntil(tester, () => remote.copies.isEmpty);
    expect(remote.copies, isEmpty);

    await tester.runAsync(() => clock.advance(const Duration(minutes: 10)));
    expect(await tester.runAsync(() => waiting), isA<Refused>());
    expect(remote.copies, isEmpty, reason: 'no new public copy');
    expect(
      (await tester.runAsync(() => me.store.entry(aid)))!.link,
      isNull,
    );
  });

  testWidgets('Tell @owner names a remote out of reach and a device signed '
      'out', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (aid, copy) = await published(tester);
    copy.comments = true;
    final tell = (await tester.runAsync(
      () => flows.tellOwner(aid, 3, 'its tests failed'),
    ))!;
    remote.failNext('comment', _offline);
    expect(
      await tester.runAsync(() => tell.post('x')),
      'Couldn’t reach clax.example. Check your connection and try again.',
    );
    remote.failNext(
      'comment',
      const ClaxApiError(
        status: 502,
        code: 'remote_signed_out',
        message: 'the remote no longer takes this device',
      ),
    );
    expect(
      await tester.runAsync(() => tell.post('x')),
      'This device is signed out of clax.example. Connect again in '
      'Settings › Sharing.',
    );
    expect(copy.posted, isEmpty);
  });

  testWidgets('a publish refused as moved shows the diverged sheet', (
    tester,
  ) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (aid, copy) = await published(tester);
    await tester.runAsync(
      () => me.api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>mine</p>'}),
      ),
    );
    final v2 = (await tester.runAsync(() => me.api.getArtifact(aid)))!;
    remote.localVersion(
      aid,
      2,
      contentSha256: v2.versions.last.contentSha256!,
      title: 'Plant log',
    );
    remote.publishThere(
      copy.raid,
      contentSha256: 'sha256:web',
      by: const PublishedBy(name: 'Sam', login: 'sam'),
    );
    await tester.pumpWidget(button((c) => flows.publish(c, aid, version: 2)));
    await tester.tap(find.text('go'));
    await settle(tester, find.text('clax.example moved on'));
    expect(find.text('clax.example moved on'), findsOneWidget);
    expect(
      find.text('@sam published v2 after this device’s v1.'),
      findsOneWidget,
    );
    // No agent here: no combine.
    expect(find.text('Ask Claude to combine them'), findsNothing);
    expect(find.text('Take the public v2 (drop mine)'), findsOneWidget);
  });

  testWidgets('the diverged sheet names the head’s note', (tester) async {
    await setUpFlows(tester);
    addTearDown(() => tmp.deleteSync(recursive: true));
    final (aid, copy) = await published(tester);
    await tester.runAsync(
      () => me.api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>mine</p>'}),
      ),
    );
    final v2 = (await tester.runAsync(() => me.api.getArtifact(aid)))!;
    remote.localVersion(
      aid,
      2,
      contentSha256: v2.versions.last.contentSha256!,
      title: 'Plant log',
    );
    remote.publishThere(
      copy.raid,
      contentSha256: 'sha256:web',
      by: const PublishedBy(name: 'Sam', login: 'sam'),
      note: 'Bigger buttons',
    );
    await tester.pumpWidget(button((c) => flows.publish(c, aid, version: 2)));
    await tester.tap(find.text('go'));
    await settle(tester, find.text('clax.example moved on'));
    expect(
      find.text('@sam published v2 (“Bigger buttons”) after this device’s v1.'),
      findsOneWidget,
    );
  });
}

final _publishButton = find.byKey(const ValueKey('publish'));
final _installButton = find.byKey(const ValueKey('install'));

/// Whether [f] finds nothing.
bool _gone(Finder f) => f.evaluate().isEmpty;
