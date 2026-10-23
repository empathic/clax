import 'dart:io';

import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/remote/checker.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/remote/sync_state.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_clock.dart';

const _offline = ClaxApiError(
  status: 0,
  code: ClaxApiError.unreachable,
  message: 'offline',
);

/// A [FakeRemoteApi] that can go offline (every read fails for want of an
/// answer) or answer a stale head.
class _Remote extends FakeRemoteApi {
  var offline = false;
  RemoteHead? stale;

  @override
  Future<List<Remote>> remotes() async {
    if (offline) throw _offline;
    return super.remotes();
  }

  @override
  Future<RemoteHead> head(String rid, String raid) async {
    if (offline) throw _offline;
    final h = await super.head(rid, raid);
    return stale ?? h;
  }

  @override
  Future<RemoteHead> anonymousHead(Uri artifactUrl) async {
    if (offline) throw _offline;
    final h = await super.anonymousHead(artifactUrl);
    return stale ?? h;
  }
}

void main() {
  late Directory tmp;
  late Install me;
  late _Remote remote;
  late Remote r;
  late FakeClock clock;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('remote-check');
    me = Install(tmp);
    await me.ready();
    remote = _Remote();
    r = remote.connected('https://clax.example');
    clock = FakeClock();
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  int heads() =>
      remote.calls.where((c) => c == 'head' || c == 'anonymousHead').length;

  /// Local app [aid], linked as a recipient to [c] at its version 1, with
  /// bundle version [held] taken.
  Future<void> linkTo(String aid, FakePublicCopy c, {int held = 3}) =>
      me.store.put(
        aid,
        LineageEntry(
          lineage: lineageA,
          role: LineageRole.installed,
          key: 'k',
          name: null,
          version: held,
          released: const [],
          capabilities: const {},
          title: 'Plant log',
          link: AppLink(
            remoteOrigin: c.origin,
            remoteId: c.rid,
            remoteArtifactId: c.raid,
            url: c.url,
            baseRemoteVersion: 1,
            takenBundleVersion: held,
            role: AppLink.recipient,
          ),
        ),
      );

  FakePublicCopy copyOn(Remote on) => remote.addCopy(
    on.id,
    contentSha256: 'sha256:${'1' * 64}',
    general: 'link',
  );

  test('checker_rate_limits_per_remote_on_injected_clock', () async {
    final c1 = copyOn(r), c2 = copyOn(r);
    await linkTo('a1', c1);
    await linkTo('a2', c2);
    final checker = LinkedChecker(api: remote, store: me.store, clock: clock);

    await checker.onForeground();
    expect(heads(), 2);
    expect(checker.status.value.keys, unorderedEquals(['a1', 'a2']));
    expect(checker.status.value['a1']!.head?.artifactId, c1.raid);

    // Another remote's apps have rounds of their own.
    await clock.advance(const Duration(minutes: 10));
    final r2 = remote.connected('https://other.example');
    await linkTo('b1', copyOn(r2));
    await checker.onForeground();
    expect(heads(), 3, reason: 'only the other remote was due');

    await clock.advance(const Duration(minutes: 4, seconds: 59));
    await checker.onForeground();
    expect(heads(), 3, reason: 'the first remote is not due before 15 min');

    await clock.advance(const Duration(seconds: 1));
    await checker.onForeground();
    expect(heads(), 5, reason: 'the first remote is due, the other not');

    await clock.advance(const Duration(minutes: 9, seconds: 59));
    await checker.onForeground();
    expect(heads(), 5);
    await clock.advance(const Duration(seconds: 1));
    await checker.onForeground();
    expect(heads(), 6, reason: 'the other remote is due 15 min after its own');
  });

  test('checker_open_bypasses_limit', () async {
    final c1 = copyOn(r);
    await linkTo('a1', c1);
    final checker = LinkedChecker(api: remote, store: me.store, clock: clock);
    await checker.onForeground();
    expect(heads(), 1);

    await checker.onOpen('a1');
    expect(heads(), 2);
    await checker.checkNow('a1');
    expect(heads(), 3);

    // Neither moved the foreground limit, nor counts as a round.
    await clock.advance(const Duration(minutes: 1));
    await checker.onForeground();
    expect(heads(), 3);
    await clock.advance(const Duration(minutes: 14));
    await checker.onForeground();
    expect(heads(), 4);
  });

  test('checker_offline_keeps_state_and_never_regresses', () async {
    final c = copyOn(r);
    final first = c.head;
    await linkTo('a1', c);
    final checker = LinkedChecker(api: remote, store: me.store, clock: clock);
    await checker.onForeground();
    expect(checker.status.value['a1']!.state, SyncState.live);

    // The copy moves on: v2 carries bundle version 4.
    remote.publishThere(
      c.raid,
      contentSha256: 'sha256:${'2' * 64}',
      bundle: FakeBundle(
        bytes: utf('b'),
        lineage: lineageA,
        bundleVersion: 4,
        contentSha256: 'sha256:${'2' * 64}',
      ),
    );
    await checker.checkNow('a1');
    expect(checker.status.value['a1']!.state, SyncState.updateWaiting);
    expect(checker.status.value['a1']!.head?.version, 2);

    // Offline: the round ends, and the state is the one the head last read
    // makes, with no failure recorded.
    remote.offline = true;
    await clock.advance(const Duration(minutes: 15));
    await checker.onForeground();
    await checker.onOpen('a1');
    final kept = checker.status.value['a1']!;
    expect(kept.state, SyncState.updateWaiting);
    expect(kept.head?.version, 2);

    // This device took v2 meanwhile: the head it holds no longer reads as
    // an update, though nothing new could be read.
    await me.store.update(
      'a1',
      (e) async => e!.copyWith(
        version: 4,
        link: () => e.link!.copyWith(
          baseRemoteVersion: 2,
          takenBundleVersion: () => 4,
        ),
      ),
    );
    await checker.checkNow('a1');
    expect(checker.status.value['a1']!.state, SyncState.live);
    expect(checker.status.value['a1']!.head?.version, 2);

    // Back online, a stale answer (the copy's v1) goes back on nothing.
    remote
      ..offline = false
      ..stale = first;
    await checker.checkNow('a1');
    expect(checker.status.value['a1']!.state, SyncState.live);
    expect(checker.status.value['a1']!.head?.version, 2);

    // And a cached head this device has passed offers nothing either, for
    // an app whose head was never read fresh.
    final again = LinkedChecker(api: remote, store: me.store, clock: clock);
    remote.stale = null;
    await again.onOpen('a1');
    expect(again.status.value['a1']!.state, SyncState.live);
  });

  test('an unmoved author whose copy was edited on the web has an update '
      'waiting, and no automatic update fetches anything', () async {
    final c = copyOn(r);
    c.versions.single.bundle = FakeBundle(
      bytes: utf('b'),
      lineage: lineageA,
      bundleVersion: 3,
      contentSha256: c.versions.single.contentSha256,
    );
    await me.store.put(
      'a1',
      LineageEntry(
        lineage: lineageA,
        role: LineageRole.author,
        key: 'k',
        name: null,
        version: 3,
        released: const [],
        capabilities: const {},
        title: 'Plant log',
        link: AppLink(
          remoteOrigin: c.origin,
          remoteId: c.rid,
          remoteArtifactId: c.raid,
          url: c.url,
          baseRemoteVersion: 1,
          takenBundleVersion: 3,
          role: AppLink.author,
          autoUpdate: true,
          published: const [3],
        ),
      ),
    );
    // An editor republishes on clax.dev: v2 has no bundle.
    remote.publishThere(c.raid, contentSha256: 'sha256:${'2' * 64}');
    final fetched = <String>[];
    final checker = LinkedChecker(
      api: remote,
      store: me.store,
      clock: clock,
      fetchUpdate: (link) async {
        fetched.add(link.remoteArtifactId);
        return const PullCancelled();
      },
      runUpdate: (aid, o) async => const UpdateRefused('not in this test'),
    );
    await checker.onOpen('a1');
    expect(checker.status.value['a1']!.head?.version, 2);
    expect(checker.status.value['a1']!.state, SyncState.updateWaiting);
    expect(fetched, isEmpty, reason: 'there is no bundle to update to');
  });

  group('automatic updates', () {
    late FakePublicCopy c;
    late String aid;
    late List<String> ran;
    late LinkedChecker checker;

    setUp(() async {
      final w = await makeBundle(lineage: lineageA, version: 3);
      c = remote.addCopy(
        r.id,
        contentSha256: w.manifest.contentSha256,
        general: 'link',
      );
      c.versions.single.bundle = FakeBundle(
        bytes: w.bytes,
        lineage: lineageA,
        bundleVersion: 3,
        contentSha256: w.manifest.contentSha256,
      );
      final puller = Puller(
        remote: remote,
        installer: me.installer,
        store: me.store,
        installId: () async => 'install',
      );
      aid = (await puller.pull(Uri.parse(c.url)) as PullInstalled).aid;
      await me.store.update(
        aid,
        (e) async =>
            e!.copyWith(link: () => e.link!.copyWith(autoUpdate: true)),
      );
      ran = [];
      checker = LinkedChecker(
        api: remote,
        store: me.store,
        clock: clock,
        live: liveFrom(me.api),
        fetchUpdate: (link) => puller.pull(
          Uri.parse(link.url),
          confirm: (_) async => false,
        ),
        runUpdate: (aid, opened) async {
          ran.add('$aid v${opened.manifest.version}');
          return const UpdateRefused('not in this test');
        },
      );
    });

    Future<void> publishThere(int version, Map<String, Object?> caps) async {
      final w = await makeBundle(
        lineage: lineageA,
        version: version,
        capabilities: caps,
        files: {'index.html': '<p>v$version</p>'},
      );
      remote.publishThere(
        c.raid,
        contentSha256: w.manifest.contentSha256,
        bundle: FakeBundle(
          bytes: w.bytes,
          lineage: lineageA,
          bundleVersion: version,
          contentSha256: w.manifest.contentSha256,
        ),
      );
    }

    test('auto_update_stops_for_new_capability', () async {
      await checker.onOpen(aid);
      expect(checker.status.value[aid]!.state, SyncState.live);

      await publishThere(4, {
        'db': <String, Object?>{},
        'net': {
          'hosts': ['api.example.com'],
        },
      });
      await checker.checkNow(aid);
      expect(ran, isEmpty, reason: 'a new net host waits for the person');
      expect(checker.status.value[aid]!.state, SyncState.updateWaiting);

      // One that asks for nothing new runs by itself.
      await publishThere(5, {'db': <String, Object?>{}});
      await checker.checkNow(aid);
      expect(ran, ['$aid v5']);
    });

    test('a landed automatic update moves the link, and the app reads '
        'live', () async {
      final real = LinkedChecker(
        api: remote,
        store: me.store,
        clock: clock,
        live: liveFrom(me.api),
        fetchUpdate: (link) => Puller(
          remote: remote,
          installer: me.installer,
          store: me.store,
          installId: () async => 'install',
        ).pull(Uri.parse(link.url), confirm: (_) async => false),
        runUpdate: (aid, OpenedBundle o) =>
            me.updater.update(aid, o.bundle, origin: o.origin),
      );
      await publishThere(4, {'db': <String, Object?>{}});
      await real.checkNow(aid);
      final e = (await me.store.entry(aid))!;
      expect(e.version, 4);
      expect(e.link!.baseRemoteVersion, 2);
      expect(e.link!.takenBundleVersion, 4);
      expect(real.status.value[aid]!.state, SyncState.live);
    });
  });
}
