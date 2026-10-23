import 'dart:io';

import 'package:clax_app/bundle/manifest.dart' show SeedDoc;
import 'package:clax_app/bundle/doc_snapshot.dart' show ownerDocs;
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/clax/api.dart' show ClaxApiError;
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';

void main() {
  late Directory tmp;
  late Install me;
  late FakeRemoteApi remote;
  late Remote r;
  late SettingsStore settings;
  late Puller puller;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('remote-pull');
    me = Install(tmp);
    await me.ready();
    remote = FakeRemoteApi();
    r = remote.connected('https://clax.example');
    settings = SettingsStore(File('${tmp.path}/settings.json'));
    puller = Puller(
      remote: remote,
      installer: me.installer,
      store: me.store,
      installId: () => ensureInstallId(settings),
    );
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  /// A public copy on [r] at version 1 whose bundle is [bytes], the head
  /// saying its content hash is [headHash] (the bundle's own when null).
  Future<FakePublicCopy> copyOf({
    String access = 'viewer',
    String? headHash,
    String general = 'link',
    List<SeedDoc> seed = const [],
  }) async {
    final w = await makeBundle(lineage: lineageA, version: 3, seed: seed);
    final hash = headHash ?? w.manifest.contentSha256;
    final c = remote.addCopy(
      r.id,
      contentSha256: hash,
      title: 'Plant log',
      access: access,
      general: general,
    );
    c.versions.single.bundle = FakeBundle(
      bytes: w.bytes,
      lineage: lineageA,
      bundleVersion: 3,
      contentSha256: hash,
    );
    return c;
  }

  /// Signs this device out of [r] (its record stays, as a pending one).
  void signOut() => remotesSignedOut(remote, r);

  test('pull_refuses_bundle_whose_hash_differs_from_head', () async {
    for (final signedIn in [true, false]) {
      final c = await copyOf(headHash: 'sha256:${'7' * 64}');
      if (!signedIn) signOut();
      remote.calls.clear();
      var asked = false;
      final out = await puller.pull(
        Uri.parse(c.url),
        confirm: (_) async => asked = true,
      );
      expect(out, isA<PullRefused>(), reason: 'signed in: $signedIn');
      expect(asked, isFalse);
      expect(remote.calls, isNot(contains('link')));
      expect(remote.calls, isNot(contains('recordInstall')));
      expect(await me.api.listArtifacts(), isEmpty);
      expect(me.store.file.existsSync(), isFalse);
      expect(c.installs, isEmpty);
    }
  });

  test('pull_installs_links_and_records_install', () async {
    final c = await copyOf();
    OpenedBundle? shown;
    final out = await puller.pull(
      Uri.parse(c.url),
      confirm: (o) async {
        shown = o;
        return true;
      },
    );
    expect(out, isA<PullInstalled>());
    final aid = (out as PullInstalled).aid;
    expect(shown?.origin?.remoteArtifactId, c.raid);
    expect(shown?.origin?.head.owner.login, 'owner');

    final e = (await me.store.entry(aid))!;
    expect(e.lineage, lineageA);
    expect(e.version, 3);
    final link = e.link!;
    expect(link.remoteOrigin, r.origin);
    expect(link.remoteId, r.id);
    expect(link.remoteArtifactId, c.raid);
    expect(link.url, c.url);
    expect(link.baseRemoteVersion, 1);
    expect(link.takenBundleVersion, 3);
    expect(link.role, AppLink.recipient);
    expect(remote.links[aid]?.raid, c.raid);
    expect(remote.links[aid]?.remoteN, 1);

    final id = (await settings.load()).installId!;
    expect(c.installs, {id});
    expect(id, matches(RegExp(r'^[0-9a-f]{64}$')));

    // Pulled again: nothing new to take, and the same install ID.
    final again = await puller.pull(Uri.parse('${c.url}/get'));
    expect(again, isA<PullHave>().having((h) => h.aid, 'aid', aid));
    expect(await me.api.listArtifacts(), hasLength(1));
  });

  test('without signing in, a pull links by the address alone', () async {
    final c = await copyOf();
    signOut();
    final out = await puller.pull(Uri.parse(c.url)) as PullInstalled;
    final link = (await me.store.entry(out.aid))!.link!;
    expect(link.remoteId, isNull);
    expect(link.role, AppLink.recipient);
    expect(remote.calls, containsAll(['anonymousHead', 'anonymousBundle']));
    expect(remote.calls, isNot(contains('link')));
    expect(c.installs, hasLength(1));
  });

  test('a signed-in pull of a copy shared by name counts the install with '
      'the token; without signing in the remote refuses to count it', () async {
    final c = await copyOf(general: 'restricted');
    final out = await puller.pull(Uri.parse(c.url));
    expect(out, isA<PullInstalled>());
    expect(remote.calls, contains('countInstall'));
    expect(remote.calls, isNot(contains('recordInstall')));
    expect(c.installs, {(await settings.load()).installId!});
    await expectLater(
      remote.recordInstall(Uri.parse(c.url), 'f' * 64),
      throwsA(isA<ClaxApiError>().having((e) => e.status, 'status', 404)),
    );
  });

  test('an owned app from the library arrives as an authored copy', () async {
    final c = await copyOf(access: 'owner', general: 'restricted');
    final app = (await remote.apps(r.id)).single;
    final out = await puller.pullFromLibrary(r.id, app) as PullInstalled;
    final e = (await me.store.entry(out.aid))!;
    expect(e.link!.role, AppLink.author);
    expect(e.link!.remoteArtifactId, c.raid);
    expect(remote.links[out.aid]?.raid, c.raid);
  });

  test('a pull brings no documents, though the bundle carries some', () async {
    final c = await copyOf(
      seed: [for (var i = 0; i < 3; i++) SeedDoc('plants/p$i', {'n': i})],
    );
    final out = await puller.pull(Uri.parse(c.url)) as PullInstalled;
    expect([await for (final d in ownerDocs(me.api, out.aid)) d], isEmpty);
    expect(me.api.calls.where((c) => c.name == 'batchDocs'), isEmpty);
  });

  test('a get the remote fails says why in the person’s words, the raw '
      'text kept as details', () async {
    final c = await copyOf();
    remote.failNext(
      'head',
      const ClaxApiError(status: 500, code: 'internal', message: 'boom'),
    );
    var out = await puller.pull(Uri.parse(c.url));
    expect(
      out,
      isA<PullRefused>()
          .having(
            (o) => o.message,
            'message',
            'Couldn’t get this app from clax.example.',
          )
          .having((o) => o.details, 'details', ['boom']),
    );
    remote.failNext(
      'head',
      const ClaxApiError(
        status: 502,
        code: 'remote_unreachable',
        message: 'https://clax.example could not be reached',
      ),
    );
    out = await puller.pull(Uri.parse(c.url));
    expect(
      (out as PullRefused).message,
      'Couldn’t reach clax.example. Check your connection and try again.',
    );
    remote.tooOld.add('https://clax.example');
    out = await puller.pull(Uri.parse(c.url));
    expect(
      (out as PullRefused).message,
      'clax.example needs an update before the Clax app can share apps.',
    );
  });

  test('an address that is not an app’s is refused', () async {
    final out = await puller.pull(Uri.parse('https://clax.example/@me/x'));
    expect(out, isA<PullRefused>());
  });
}

/// Turns [r]'s sign-in on [remote] back into a pending one.
void remotesSignedOut(FakeRemoteApi remote, Remote r) =>
    remote.remotesById[r.id] = Remote(
      id: r.id,
      origin: r.origin,
      name: r.name,
      status: 'pending',
    );
