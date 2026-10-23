import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/remote_api.dart';
import 'package:flutter_test/flutter_test.dart';

String _h(int i) => 'sha256:${i.toRadixString(16).padLeft(64, '0')}';

Matcher _refused(String code) =>
    throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', code));

void main() {
  late FakeRemoteApi api;
  late Remote r;
  setUp(() {
    api = FakeRemoteApi();
    r = api.connected('https://clax.example');
  });

  test('a publish makes the next public version, comments off and '
      'restricted, and is refused once the copy moved', () async {
    api
      ..localVersion('a1', 3, contentSha256: _h(3))
      ..localVersion('a1', 5, contentSha256: _h(5));
    final p1 = await api.publish('a1', version: 3);
    expect(p1.remoteVersion, 1);
    final c = api.copies[p1.remoteArtifactId]!;
    expect(c.comments, isFalse);
    expect(c.general, 'restricted');
    expect((await api.publish('a1', version: 5)).remoteVersion, 2);
    // The same local version again is a new public version.
    expect((await api.publish('a1', version: 3)).remoteVersion, 3);

    api.publishThere(c.raid, contentSha256: _h(9));
    await expectLater(
      api.publish('a1', version: 5),
      throwsA(isA<RemoteMoved>().having((e) => e.remoteVersion, 'at', 4)),
    );
  });

  test('an unprogrammed publish or poll throws', () async {
    await expectLater(api.publish('a1', version: 1), throwsStateError);
    final pending = (await api.addRemote('other.example')).remote;
    await expectLater(api.poll(pending.id), throwsStateError);
    api.signInsWait = true;
    expect((await api.poll(pending.id)).status, 'pending');
    api.approve(pending.id);
    expect((await api.poll(pending.id)).connected, isTrue);
  });

  test('a new sign-in replaces the pending one at its origin, as the core '
      'does', () async {
    final first = (await api.addRemote('a.example')).remote;
    final second = (await api.addRemote('a.example')).remote;
    expect(api.remotesById.keys, containsAll([r.id, second.id]));
    expect(api.remotesById, isNot(contains(first.id)));
  });

  test('a machine name over 64 characters, or with a control character, is '
      'refused as the remote refuses it', () async {
    await expectLater(
      api.addRemote('a.example', machine: 'x' * 65),
      throwsA(isA<ClaxApiError>().having((e) => e.status, 'status', 502)),
    );
    await expectLater(
      api.addRemote('a.example', machine: 'Clax on a\tb'),
      throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'remote_refused')),
    );
    expect((await api.addRemote('a.example', machine: ' ${'x' * 64} ')).remote.status, 'pending');
  });

  test('a public copy is reached through a later sign-in at its origin', () async {
    final c = api.addCopy(r.id, contentSha256: _h(1));
    await api.removeRemote(r.id);
    final again = api.connected(r.origin);
    expect((await api.head(again.id, c.raid)).artifactId, c.raid);
    final other = api.connected('https://other.example');
    await expectLater(api.head(other.id, c.raid), throwsA(isA<ClaxApiError>()));
  });

  test('a denied or expired sign-in says so and is forgotten', () async {
    final denied = (await api.addRemote('a.example')).remote;
    api.deny(denied.id);
    expect((await api.poll(denied.id)).status, 'denied');
    final lapsed = (await api.addRemote('b.example')).remote;
    api.expire(lapsed.id);
    expect((await api.poll(lapsed.id)).status, 'expired');
    expect(await api.remotes(), [r]);
  });

  test('bundles: the version must exist and the bundle version go up; the '
      'same bytes again succeed', () async {
    api.localVersion('a1', 1, contentSha256: _h(1));
    final p = await api.publish('a1', version: 1);
    final bytes = Uint8List.fromList([1, 2]);
    Future<void> put(int n, int bv, Uint8List b, {String lineage = 'l_a'}) =>
        api.putBundle(
          'a1',
          r.id,
          n,
          b,
          lineage: lineage,
          bundleVersion: bv,
          contentSha256: _h(n),
        );
    await expectLater(put(2, 1, bytes), _refused('not_found'));
    await put(1, 4, bytes);
    await put(1, 4, bytes);
    await expectLater(put(1, 4, Uint8List(1)), _refused('bundle_exists'));
    api.localVersion('a1', 2, contentSha256: _h(2));
    await api.publish('a1', version: 2);
    await expectLater(put(2, 4, bytes), _refused('bundle_version'));
    await expectLater(put(2, 5, bytes, lineage: 'l_b'), _refused('bundle_lineage'));
    await expectLater(
      api.putBundle(
        'a1',
        r.id,
        2,
        bytes,
        lineage: 'l_a',
        bundleVersion: 5,
        contentSha256: _h(1),
      ),
      _refused('bundle_mismatch'),
    );
    final h = await api.head(r.id, p.remoteArtifactId);
    expect(h.bundle!.bundleVersion, 4);
    expect(h.webMade, isTrue, reason: 'version 2 carries no bundle yet');
  });

  test('a restricted copy is not found without signing in', () async {
    final c = api.addCopy(r.id, contentSha256: _h(1), general: 'restricted');
    await expectLater(api.anonymousHead(Uri.parse(c.url)), _refused('not_found'));
    c.general = 'link';
    expect((await api.anonymousHead(Uri.parse(c.url))).version, 1);
  });

  test('only the owner shares, and a viewer may not publish', () async {
    final c = api.addCopy(r.id, contentSha256: _h(1));
    await expectLater(api.setComments(r.id, c.raid, false), _refused('not_permitted'));
    expect((await api.sharing(r.id, c.raid)).canShare, isFalse);
    api.localVersion('a1', 1, contentSha256: _h(1));
    await api.link(
      r.id,
      'a1',
      RemoteApp(
        artifactId: c.raid,
        title: 'App',
        url: c.url,
        version: 1,
        bundleVersion: 1,
        access: 'viewer',
        general: 'link',
      ),
      1,
    );
    await expectLater(api.publish('a1', version: 1), _refused('not_permitted'));
    await expectLater(
      api.putBundle(
        'a1',
        r.id,
        1,
        Uint8List.fromList([1]),
        lineage: 'l_a',
        bundleVersion: 1,
        contentSha256: _h(1),
      ),
      _refused('not_permitted'),
    );
  });

  test('failNext makes the next call of a method throw, once', () async {
    const offline = ClaxApiError(
      status: 0,
      code: ClaxApiError.unreachable,
      message: 'offline',
    );
    api.failNext('remotes', offline);
    await expectLater(api.remotes(), throwsA(same(offline)));
    expect(await api.remotes(), hasLength(1));
  });

  test('a hosted Clax too old for the app answers every call that reaches '
      'it 502 remote_too_old, naming its origin; the core\'s own calls work', () async {
    final c = api.addCopy(r.id, contentSha256: _h(1));
    api.tooOld.add('https://clax.example');
    final tooOld = throwsA(
      isA<ClaxApiError>()
          .having((e) => e.status, 'status', 502)
          .having((e) => e.code, 'code', 'remote_too_old')
          .having((e) => e.message, 'message', contains('https://clax.example')),
    );
    await expectLater(api.addRemote('https://clax.example'), tooOld);
    await expectLater(api.head(r.id, c.raid), tooOld);
    await expectLater(api.apps(r.id), tooOld);
    await expectLater(api.sharing(r.id, c.raid), tooOld);
    await expectLater(api.anonymousHead(Uri.parse(c.url)), tooOld);
    expect(await api.remotes(), hasLength(1));
  });
}
