import 'dart:async';
import 'dart:io';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_clock.dart' show FakeClock;
import '../support/settle.dart';

const _offline = ClaxApiError(
  status: 0,
  code: ClaxApiError.unreachable,
  message: 'offline',
);

void main() {
  late Directory tmp;
  late Install me;
  late FakeRemoteApi remote;
  late Remote r;
  late FakeClock clock;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('remote-publish');
    me = Install(tmp);
    await me.ready();
    remote = FakeRemoteApi();
    r = remote.connected('https://clax.example');
    clock = FakeClock();
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  Publisher publisher([FakeClock? on]) => Publisher(
    remote: remote,
    exporter: me.exporter,
    store: me.store,
    clock: on ?? clock,
  );

  /// An app with versions 1 and 2, each programmed on the fake as what
  /// its publish sends.
  Future<String> makeApp() async {
    final made = await me.api.createArtifact(
      PublishRequest(
        title: 'Plant log',
        icon: 'leaf',
        entries: {'index.html': FileEntry.text('<p>v1</p>')},
      ),
    );
    final aid = made.artifact.id;
    await me.api.publish(
      aid,
      PublishRequest(ifVersion: 1, files: {'index.html': '<p>v2</p>'}),
    );
    for (final v in (await me.api.getArtifact(aid)).versions) {
      remote.localVersion(
        aid,
        v.n,
        contentSha256: v.contentSha256!,
        title: 'Plant log',
      );
    }
    return aid;
  }

  Future<String> hashOf(String aid, int n) async =>
      (await me.api.getArtifact(aid)).versions
          .firstWhere((v) => v.n == n)
          .contentSha256!;

  group('Publish fork', () {
    /// Sam's Plant log installed here, changed since: v1 is the install,
    /// v2 the person's change; each programmed on the fake.
    Future<String> installChanged() async {
      final v1 = await makeBundle(lineage: lineageA);
      final aid =
          (await me.installer.install(await me.installer.open(v1.bytes))).aid;
      await me.api.publish(
        aid,
        PublishRequest(
          ifVersion: me.api.currentVersion(aid),
          files: {'index.html': '<p>v1</p><p>mine</p>'},
        ),
      );
      for (final v in (await me.api.getArtifact(aid)).versions) {
        remote.localVersion(
          aid,
          v.n,
          contentSha256: v.contentSha256!,
          title: 'Plant log',
        );
      }
      return aid;
    }

    test('a copy linked to the original goes up as a new app under its new '
        'name, made from the original, and is relinked to it', () async {
      final aid = await installChanged();
      final original = remote.addCopy(
        r.id,
        contentSha256: 'sha256:original',
        title: 'Plant log',
      );
      await me.store.update(
        aid,
        (e) async => e!.copyWith(
          link: () => AppLink(
            remoteOrigin: r.origin,
            remoteId: r.id,
            remoteArtifactId: original.raid,
            url: original.url,
            baseRemoteVersion: 1,
            takenBundleVersion: 1,
            role: AppLink.recipient,
          ),
        ),
      );
      final was = (await me.store.entry(aid))!;
      final out = await publisher().publishFork(
        aid,
        name: 'Plant log (yours)',
        autoPublish: true,
      );
      expect(out, isA<Published>());
      final fork = remote.copies.values.firstWhere((c) => c != original);
      expect(fork.title, 'Plant log (yours)');
      expect(fork.forkedFrom, original.raid);
      expect(fork.general, 'restricted');
      expect(fork.comments, isFalse);
      expect(original.versions, hasLength(1));
      // The bundle is a fork: a new lineage under this install's key,
      // forked from the original's, carrying the new name.
      final b = await BundleReader.read(fork.versions.single.bundle!.bytes);
      expect(b.manifest.lineage, isNot(was.lineage));
      expect(b.manifest.title, 'Plant log (yours)');
      expect(b.manifest.forkedFrom?.lineage, was.lineage);
      // The app is the person's own now, linked to its own copy.
      final e = (await me.store.entry(aid))!;
      expect(e.role, LineageRole.author);
      expect(e.link!.remoteArtifactId, fork.raid);
      expect(e.link!.isAuthor, isTrue);
      expect(e.link!.autoPublish, isTrue);
      expect(remote.links[aid]!.raid, fork.raid);
    });

    test('an app installed from a file goes up on the remote named, made '
        'from nothing public', () async {
      final aid = await installChanged();
      final out = await publisher().publishFork(
        aid,
        name: '  Plant log (yours) ',
        rid: r.id,
      );
      expect(out, isA<Published>());
      final fork = remote.copies.values.single;
      expect(fork.title, 'Plant log (yours)');
      expect(fork.forkedFrom, isNull);
      expect((await me.store.entry(aid))!.link!.remoteArtifactId, fork.raid);
    });

    test('an app of the person’s own, or no name, is refused', () async {
      final own = await makeApp();
      expect(
        await publisher().publishFork(own, name: 'Mine', rid: r.id),
        isA<Refused>(),
      );
      final aid = await installChanged();
      expect(
        await publisher().publishFork(aid, name: '  ', rid: r.id),
        isA<Refused>(),
      );
      expect(remote.copies, isEmpty);
    });
  });

  test('publish_exports_pushes_uploads_and_links', () async {
    final aid = await makeApp();
    final pub = publisher();

    final first = await pub.publish(aid, version: 1);
    expect(first, isA<Published>());
    first as Published;
    expect(first.remoteVersion, 1);
    expect(remote.calls.where((c) => c != 'remotes'), ['publish', 'putBundle']);
    final copy = remote.copies.values.single;
    expect(first.url, copy.url);

    // The bundle is version 1's files, byte for byte what a pull gets.
    final b1 = await BundleReader.read(copy.versions.single.bundle!.bytes);
    expect(b1.manifest.contentSha256, await hashOf(aid, 1));
    expect(b1.manifest.seed, isNull);
    final e1 = (await me.store.entry(aid))!;
    expect(e1.lineage, b1.manifest.lineage);
    expect(e1.role, LineageRole.author);
    final l1 = e1.link!;
    expect(l1.role, AppLink.author);
    expect(l1.remoteId, r.id);
    expect(l1.remoteOrigin, r.origin);
    expect(l1.remoteArtifactId, copy.raid);
    expect(l1.url, copy.url);
    expect(l1.baseRemoteVersion, 1);
    expect(l1.pendingBundle, isNull);
    expect(l1.published, [1]);

    // The next publish continues the lineage past the last bundle.
    final second = await pub.publish(aid, version: 2) as Published;
    expect(second.remoteVersion, 2);
    final b2 = await BundleReader.read(copy.versions[1].bundle!.bytes);
    expect(b2.manifest.lineage, b1.manifest.lineage);
    expect(b2.manifest.version, greaterThan(b1.manifest.version));
    expect(b2.manifest.contentSha256, await hashOf(aid, 2));
    final l2 = (await me.store.entry(aid))!.link!;
    expect(l2.baseRemoteVersion, 2);
    expect(l2.published, [1, 2]);
    expect(Directory('${tmp.path}/support/publishing').listSync(), isEmpty);
  });

  test('publish_bundle_upload_resumes_after_restart', () async {
    final aid = await makeApp();
    remote
      ..failNext('putBundle', _offline)
      ..failNext('putBundle', _offline);
    final pub = publisher();
    var done = false;
    final waits = Completer<void>();
    pub.waiting.addListener(() {
      if (pub.waiting.value.contains(aid) && !waits.isCompleted) {
        waits.complete();
      }
    });
    final going = pub.publish(aid, version: 2).whenComplete(() => done = true);
    await waits.future;
    expect(pub.waiting.value, {aid});
    expect(remote.calls.where((c) => c == 'putBundle'), hasLength(1));

    // Backoff on the injected clock: nothing before the first retry is due.
    await clock.advance(const Duration(seconds: 1));
    expect(remote.calls.where((c) => c == 'putBundle'), hasLength(1));
    await clock.advance(const Duration(seconds: 1));
    expect(remote.calls.where((c) => c == 'putBundle'), hasLength(2));
    expect(done, isFalse);

    final copy = remote.copies.values.single;
    expect(copy.versions.single.bundle, isNull);
    final pending = (await me.store.entry(aid))!.link!.pendingBundle!;
    expect(pending.remoteVersion, 1);
    expect(pending.contentSha256, await hashOf(aid, 2));

    // The app is killed here; the next launch resumes the upload.
    await publisher(FakeClock()).resume();
    final landed = copy.versions.single.bundle!;
    expect(landed.contentSha256, await hashOf(aid, 2));
    expect((await me.store.entry(aid))!.link!.pendingBundle, isNull);
    expect(Directory('${tmp.path}/support/publishing').listSync(), isEmpty);

    // The first run's next retry finds the bundle landed and sends
    // nothing more.
    await clock.advance(const Duration(minutes: 1));
    expect(done, isTrue);
    expect(await going, isA<Published>());
    expect(remote.calls.where((c) => c == 'putBundle'), hasLength(3));
    expect(copy.versions, hasLength(1));
    expect(pub.waiting.value, isEmpty);
  });

  test('publish_after_disconnect_and_reconnect_lands_on_the_same_copy', () async {
    final aid = await makeApp();
    final pub = publisher();
    final first = await pub.publish(aid, version: 1) as Published;
    final copy = remote.copies.values.single;

    // Disconnect forgets the core's remote and its links; signing in again
    // gives the same hosted Clax a new remote ID.
    await remote.removeRemote(r.id);
    expect(remote.links, isEmpty);
    final again = remote.connected(r.origin);
    expect(again.id, isNot(r.id));

    final second = await pub.publish(aid, version: 2);
    expect(second, isA<Published>());
    second as Published;
    expect(remote.copies, hasLength(1), reason: 'no second public copy');
    expect(second.url, first.url);
    expect(second.remoteVersion, 2);
    expect(copy.versions, hasLength(2));
    expect(copy.versions[1].bundle, isNotNull);
    final l = (await me.store.entry(aid))!.link!;
    expect(l.remoteId, again.id);
    expect(l.baseRemoteVersion, 2);
    expect(remote.links[aid]?.rid, again.id);
  });

  test('while disconnected a publish is refused and nothing is made', () async {
    final aid = await makeApp();
    final pub = publisher();
    await pub.publish(aid, version: 1);
    await remote.removeRemote(r.id);
    final out = await pub.publish(aid, version: 2);
    expect(out, isA<Refused>());
    expect(remote.copies.values.single.versions, hasLength(1));
  });

  test('a pending bundle survives Disconnect and lands after the next '
      'sign-in', () async {
    final aid = await makeApp();
    remote.failNext('putBundle', _offline);
    final pub = publisher();
    final going = pub.publish(aid, version: 1);
    // Its bundle written, and the upload waiting to try again.
    await waitUntil(
      () async =>
          pub.waiting.value.contains(aid) &&
          (await me.store.entry(aid))?.link?.pendingBundle != null,
    );
    expect((await me.store.entry(aid))!.link!.pendingBundle, isNotNull);

    // Disconnected before the retry: the upload finds no remote there and
    // keeps the bundle.
    await remote.removeRemote(r.id);
    await clock.advance(const Duration(seconds: 2));
    final out = await going;
    expect(out, isA<Refused>());
    expect((await me.store.entry(aid))!.link!.pendingBundle, isNotNull);
    expect(Directory('${tmp.path}/support/publishing').listSync(), hasLength(1));
    final copy = remote.copies.values.single;
    expect(copy.versions.single.bundle, isNull);

    // A launch while still signed out keeps it too.
    await publisher(FakeClock()).resume();
    expect((await me.store.entry(aid))!.link!.pendingBundle, isNotNull);

    // Signed in again: it is linked again and lands.
    final again = remote.connected(r.origin);
    await pub.links.relinkAll(again);
    await pub.resume();
    expect(copy.versions.single.bundle, isNotNull);
    expect((await me.store.entry(aid))!.link!.pendingBundle, isNull);
    expect(Directory('${tmp.path}/support/publishing').listSync(), isEmpty);
  });

  test('publish_moved_returns_moved', () async {
    final aid = await makeApp();
    final pub = publisher();
    await pub.publish(aid, version: 1);
    final copy = remote.copies.values.single;
    remote.publishThere(copy.raid, contentSha256: 'sha256:${'9' * 64}');
    remote.calls.clear();

    final before = (await me.store.entry(aid))!.toJson();
    final out = await pub.publish(aid, version: 2);
    expect(out, isA<Moved>().having((m) => m.remoteVersion, 'at', 2));
    expect(remote.calls.where((c) => c != 'remotes'), ['head', 'publish']);
    expect(copy.versions, hasLength(2));
    // Nothing was published, so the lineage records no release either.
    expect((await me.store.entry(aid))!.toJson(), before);
  });

  test('a publish whose push is refused leaves the lineage as it was, so '
      'another editor\'s next bundle reads as an update here', () async {
    final aid = await makeApp();
    final pub = publisher();
    expect(await pub.publish(aid, version: 1), isA<Published>());
    final copy = remote.copies.values.single;

    // A publish the remote refuses.
    remote.failNext(
      'publish',
      const ClaxApiError(status: 400, code: 'nope', message: 'refused'),
    );
    expect(await pub.publish(aid, version: 2), isA<Refused>());

    // Another device of the same account gets the copy and publishes it.
    final tmp2 = Directory.systemTemp.createTempSync('remote-publish-b');
    addTearDown(() => tmp2.deleteSync(recursive: true));
    final other = Install(tmp2, keySeed: 2);
    await other.ready();
    // Its local IDs must not be the first install's.
    for (var i = 0; i < 3; i++) {
      other.api.seedArtifact('Filler $i', {'index.html': '<p>$i</p>'});
    }
    final got = await Puller(
      remote: remote,
      installer: other.installer,
      store: other.store,
      installId: () async => 'b' * 64,
    ).pullFromLibrary(r.id, (await remote.apps(r.id)).single);
    final bAid = (got as PullInstalled).aid;
    expect(bAid, isNot(aid));
    await other.api.publish(
      bAid,
      PublishRequest(ifVersion: 1, files: {'index.html': '<p>b</p>'}),
    );
    final bHash = (await other.api.getArtifact(bAid)).versions.last;
    remote.localVersion(bAid, 2, contentSha256: bHash.contentSha256!);
    final bPub = Publisher(
      remote: remote,
      exporter: other.exporter,
      store: other.store,
      clock: FakeClock(),
    );
    expect(await bPub.publish(bAid, version: 2), isA<Published>());
    expect(copy.versions, hasLength(2));

    // Here, the other device's version is an update.
    final pulled = await Puller(
      remote: remote,
      installer: me.installer,
      store: me.store,
      installId: () async => 'a' * 64,
    ).pull(Uri.parse(copy.url));
    expect(pulled, isA<PullUpdate>());
  });

  test('a publish while an earlier bundle is pending lands that one first, '
      'so each public version keeps its own bundle', () async {
    final aid = await makeApp();
    remote.failNext('putBundle', _offline);
    // The first run publishes v1, whose upload waits for a connection, and
    // ends.
    expect(await publisher().publishOrWait(aid, version: 1), isNull);
    expect((await me.store.entry(aid))!.link!.pendingBundle, isNotNull);

    final out = await publisher(FakeClock()).publish(aid, version: 2);
    expect(out, isA<Published>());
    final copy = remote.copies.values.single;
    expect(copy.versions, hasLength(2));
    final b1 = await BundleReader.read(copy.versions[0].bundle!.bytes);
    final b2 = await BundleReader.read(copy.versions[1].bundle!.bytes);
    expect(b1.manifest.contentSha256, await hashOf(aid, 1));
    expect(b2.manifest.contentSha256, await hashOf(aid, 2));
  });

  test('publishOrWait answers as soon as the publish waits for a connection; '
      'the publish goes on and lands by itself', () async {
    final aid = await makeApp();
    final pub = publisher();
    expect(await pub.publishOrWait(aid, version: 1), isA<Published>());

    const unavailable = ClaxApiError(
      status: 503,
      code: 'unavailable',
      message: 'clax.example is down',
    );
    remote
      ..failNext('publish', unavailable)
      ..failNext('publish', unavailable);
    final landed = pub.published.first;
    expect(await pub.publishOrWait(aid, version: 2), isNull);
    expect(pub.waiting.value, {aid});
    expect(remote.copies.values.single.versions, hasLength(1));

    // A later ask for the same version while it waits answers at once,
    // and joins that publish.
    expect(await pub.publishOrWait(aid, version: 2), isNull);

    await clock.advance(const Duration(seconds: 2));
    expect(remote.copies.values.single.versions, hasLength(1));
    await clock.advance(const Duration(seconds: 4));
    final p = await landed;
    expect((p.aid, p.version, p.remoteVersion), (aid, 2, 2));
    expect(pub.waiting.value, isEmpty);
    await clock.advance(const Duration(minutes: 10));
    expect(remote.copies.values.single.versions, hasLength(2));
  });

  test('Publish as a new app waits for a publish still trying to land on '
      'the old copy, and links the app to the new copy alone', () async {
    final aid = await makeApp();
    final pub = publisher();
    expect(await pub.publish(aid, version: 1), isA<Published>());
    final old = remote.copies.values.single;

    remote.failNext('publish', _offline);
    final waiting = pub.publish(aid, version: 2);
    await waitUntil(() => pub.waiting.value.contains(aid));
    expect(pub.waiting.value, {aid});
    // Queued behind the waiting publish however soon it is asked for.
    final asNew = pub.publishAsNewApp(aid, version: 2);
    expect(remote.links[aid]?.raid, old.raid, reason: 'not unlinked yet');

    await clock.advance(const Duration(seconds: 2));
    expect(await waiting, isA<Published>());
    expect(old.versions, hasLength(2), reason: 'the retry went to the old copy');
    final out = await asNew;
    expect(out, isA<Published>());
    expect(remote.copies, hasLength(2));
    final fresh = remote.copies.values.firstWhere((c) => c.raid != old.raid);
    final link = (await me.store.entry(aid))!.link!;
    expect(link.remoteArtifactId, fresh.raid);
    expect(link.url, fresh.url);
    expect(remote.links[aid]?.raid, fresh.raid);
  });

  test('stop ends a publish waiting to try again, and one queued behind it, '
      'before they push', () async {
    final aid = await makeApp();
    final pub = publisher();
    expect(await pub.publish(aid, version: 1), isA<Published>());
    remote.failNext('publish', _offline);
    final first = pub.publish(aid, version: 2);
    await waitUntil(() => pub.waiting.value.contains(aid));
    final second = pub.publish(aid, version: 1, includeData: true);
    expect(pub.waiting.value, {aid});

    pub.stop(aid);
    expect(await first, isA<Refused>());
    expect(await second, isA<Refused>());
    expect(pub.waiting.value, isEmpty);
    await clock.advance(const Duration(minutes: 10));
    expect(remote.calls.where((c) => c == 'publish'), hasLength(2));
    expect(remote.copies.values.single.versions, hasLength(1));

    // A publish asked for after it goes as usual.
    expect(await pub.publish(aid, version: 2), isA<Published>());
  });

  test('the core’s remote_refused, remote_signed_out and remote_too_old are '
      'final refusals in the person’s words, never a wait for a connection', () async {
    final aid = await makeApp();
    final pub = publisher();
    expect(await pub.publish(aid, version: 1), isA<Published>());
    final lines = {
      'remote_refused': 'The publish could not finish.',
      'remote_signed_out':
          'This device is signed out of clax.example. Connect again in '
          'Settings › Sharing.',
      'remote_too_old':
          'clax.example needs an update before the Clax app can publish '
          'apps.',
      'not_permitted':
          'This account isn’t allowed to do that on clax.example.',
    };
    for (final MapEntry(key: code, value: line) in lines.entries) {
      remote.failNext(
        'publish',
        ClaxApiError(
          status: code == 'not_permitted' ? 403 : 502,
          code: code,
          message: 'refused: $code',
        ),
      );
      final out = await pub.publishOrWait(aid, version: 2);
      expect(out, isA<Refused>(), reason: code);
      expect((out as Refused).message, line, reason: code);
      expect(out.details, ['refused: $code'], reason: 'the raw text, kept');
      expect(pub.waiting.value, isEmpty, reason: code);
    }
    // Nothing waits in the way: the next publish lands.
    expect(await pub.publish(aid, version: 2), isA<Published>());
  });

  test('a publish of a version already on its way joins it, whoever asked; '
      'one asked after it landed makes a new public version', () async {
    final aid = await makeApp();
    final pub = publisher();
    expect(await pub.publish(aid, version: 1), isA<Published>());
    const unavailable = ClaxApiError(
      status: 503,
      code: 'unavailable',
      message: 'clax.example is down',
    );
    remote
      ..failNext('publish', unavailable)
      ..failNext('publish', unavailable);
    // The publish sheet publishes v2; Publish v2 and reply asks for it too.
    final sheet = pub.publish(aid, version: 2);
    expect(await pub.publishOrWait(aid, version: 2), isNull);
    final again = pub.publish(aid, version: 2);
    await clock.advance(const Duration(minutes: 10));
    final one = await sheet as Published;
    expect(identical(await again, one), isTrue, reason: 'joined');
    expect(remote.copies.values.single.versions, hasLength(2));

    // Publishing v2 again once it is public is a new public version (D6).
    final republished = await pub.publish(aid, version: 2) as Published;
    expect(republished.remoteVersion, 3);
    expect(remote.copies.values.single.versions, hasLength(3));
    expect((await me.store.entry(aid))!.link!.published, [1, 2, 2]);
  });

  group('a publish of a version on its way is joined by what it pushes', () {
    const unavailable = ClaxApiError(
      status: 503,
      code: 'unavailable',
      message: 'clax.example is down',
    );

    /// An app at v1 on the copy, whose next two pushes get a 503.
    Future<(String, Publisher)> linkedAndDown() async {
      final aid = await makeApp();
      final pub = publisher();
      expect(await pub.publish(aid, version: 1, rid: r.id), isA<Published>());
      remote
        ..failNext('publish', unavailable)
        ..failNext('publish', unavailable);
      return (aid, pub);
    }

    test('the sheet’s remote and choice join Publish vN and reply', () async {
      final (aid, pub) = await linkedAndDown();
      expect(await pub.publishOrWait(aid, version: 2), isNull);
      final sheet = pub.publish(aid, version: 2, rid: r.id, autoPublish: true);
      await clock.advance(const Duration(minutes: 10));
      expect(await sheet, isA<Published>());
      expect(remote.copies.values.single.versions, hasLength(2));
      final link = (await me.store.entry(aid))!.link!;
      expect(link.published, [1, 2]);
      expect(link.autoPublish, isTrue, reason: 'the joined ask’s choice holds');
    });

    test('Include my data starts its own publish unless the one on its way '
        'includes data; a plain ask joins one that does', () async {
      final (aid, pub) = await linkedAndDown();
      me.api.putDoc(aid, 'plants/p1', {'name': 'fern'});
      final plain = pub.publish(aid, version: 2, rid: r.id);
      final data = pub.publish(aid, version: 2, rid: r.id, includeData: true);
      final again = pub.publish(aid, version: 2);
      await clock.advance(const Duration(minutes: 10));
      final outs = await Future.wait([plain, data, again]);
      expect(identical(outs[1], outs[2]), isTrue);
      final copy = remote.copies.values.single;
      expect(copy.versions, hasLength(3));
      final b = await BundleReader.read(copy.versions[2].bundle!.bytes);
      expect(b.manifest.seed?.docs, 1);
    });
  });

  test('every publish tells how it ended, also one publishOrWait left to '
      'go on', () async {
    final aid = await makeApp();
    final pub = publisher();
    final ended = <(String, int, Type)>[];
    pub.ended.listen((e) => ended.add((e.aid, e.version, e.outcome.runtimeType)));
    expect(await pub.publish(aid, version: 1), isA<Published>());
    remote.failNext(
      'publish',
      const ClaxApiError(status: 503, code: 'unavailable', message: 'down'),
    );
    final second = pub.ended.firstWhere((e) => e.version == 2);
    expect(await pub.publishOrWait(aid, version: 2), isNull);
    // Meanwhile the copy is changed on the web: the push is refused as
    // moved.
    final copy = remote.copies.values.single;
    remote.publishThere(copy.raid, contentSha256: 'sha256:${'9' * 64}');
    await clock.advance(const Duration(minutes: 1));
    expect((await second).outcome, isA<Moved>());
    expect(ended, [(aid, 1, Published), (aid, 2, Moved)]);
  });

  test('two publishes of one app at once run one after the other', () async {
    final aid = await makeApp();
    final pub = publisher();
    final both = await Future.wait([
      pub.publish(aid, version: 1),
      pub.publish(aid, version: 2),
    ]);
    expect(both[0], isA<Published>().having((p) => p.remoteVersion, 'n', 1));
    expect(both[1], isA<Published>().having((p) => p.remoteVersion, 'n', 2));
    final copy = remote.copies.values.single;
    final b1 = await BundleReader.read(copy.versions[0].bundle!.bytes);
    final b2 = await BundleReader.read(copy.versions[1].bundle!.bytes);
    expect(b1.manifest.contentSha256, await hashOf(aid, 1));
    expect(b2.manifest.contentSha256, await hashOf(aid, 2));
    expect(b2.manifest.version, greaterThan(b1.manifest.version));
    final link = (await me.store.entry(aid))!.link!;
    expect(link.baseRemoteVersion, 2);
    expect(link.published, [1, 2]);
  });

  test('an app whose files use assets is refused in one sentence', () async {
    final aid = await makeApp();
    remote.usesAssets.add(aid);
    final out = await publisher().publish(aid, version: 1);
    expect(
      out,
      isA<Refused>().having(
        (o) => o.message,
        'message',
        'This app uses files Clax can’t share yet.',
      ),
    );
    expect(await me.store.entry(aid), isNull);
  });

  test('Include my data sends the documents once and is not '
      'remembered', () async {
    final aid = await makeApp();
    me.api.putDoc(aid, 'plants/p1', {'name': 'fern'});
    final pub = publisher();
    await pub.publish(aid, version: 2, includeData: true);
    final copy = remote.copies.values.single;
    final b = await BundleReader.read(copy.versions.single.bundle!.bytes);
    expect(b.manifest.seed?.docs, 1);
    await pub.publish(aid, version: 2);
    final again = await BundleReader.read(copy.versions[1].bundle!.bytes);
    expect(again.manifest.seed, isNull);
  });

  test('a publish goes past the copy’s latest bundle version (the head '
      'floor)', () async {
    final aid = await makeApp();
    final pub = publisher();
    await pub.publish(aid, version: 1) as Published;
    final copy = remote.copies.values.single;
    // The copy's last bundle is at 9, as another editor's publishes leave
    // it, while this device's lineage entry stays at its own release.
    final b1 = copy.versions.single.bundle!;
    copy.versions.single.bundle = FakeBundle(
      bytes: b1.bytes,
      lineage: b1.lineage,
      bundleVersion: 9,
      contentSha256: b1.contentSha256,
    );
    expect((await me.store.entry(aid))!.version, lessThan(9));

    final out = await pub.publish(aid, version: 2);
    expect(out, isA<Published>());
    final b2 = copy.versions[1].bundle!;
    expect(b2.bundleVersion, 10);
    expect((await BundleReader.read(b2.bytes)).manifest.version, 10);
  });

  test('a landed change publishes by itself only with Publish new versions '
      'Automatically, and each landing publish is told', () async {
    final aid = await makeApp();
    final pub = publisher();
    final told = <(String, int, int)>[];
    final sub = pub.published.listen(
      (p) => told.add((p.aid, p.version, p.remoteVersion)),
    );
    addTearDown(sub.cancel);

    // Not linked: nothing.
    expect(await pub.autoPublish(aid, 1), isNull);
    expect(remote.calls, isNot(contains('publish')));

    await pub.publish(aid, version: 1);
    await waitUntil(() => told.isNotEmpty);
    expect(told, [(aid, 1, 1)]);
    // Ask me (the default): nothing.
    expect(await pub.autoPublish(aid, 2), isNull);
    expect(remote.calls.where((c) => c == 'publish'), hasLength(1));

    await me.store.update(
      aid,
      (e) async => e!.copyWith(link: () => e.link!.copyWith(autoPublish: true)),
    );
    // A version public already is not published again.
    expect(await pub.autoPublish(aid, 1), isNull);
    final out = await pub.autoPublish(aid, 2);
    expect(out, isA<Published>());
    await waitUntil(() => told.length > 1);
    expect(told, [(aid, 1, 1), (aid, 2, 2)]);
    expect((await me.store.entry(aid))!.link!.published, [1, 2]);
  });
}
