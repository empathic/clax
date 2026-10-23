import 'dart:math';

import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/sync_state.dart';
import 'package:flutter_test/flutter_test.dart';

const _origin = 'https://clax.example';
const _raid = 'ra1';

String _h(int i) => 'sha256:${i.toRadixString(16).padLeft(64, '0')}';

const _by = PublishedBy(name: 'Sam', login: 'sam');

AppLink _link(String role, {int base = 3, int? taken, String? merged}) =>
    AppLink(
  remoteOrigin: _origin,
  remoteId: 'r1',
  remoteArtifactId: _raid,
  url: '$_origin/a/$_raid',
  baseRemoteVersion: base,
  takenBundleVersion: taken,
  mergedSha256: merged,
  role: role,
);

LineageEntry _entry({
  int version = 3,
  List<String>? released,
  bool stopped = false,
  AppLink? link,
}) => LineageEntry(
  lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
  role: link?.isAuthor ?? true ? LineageRole.author : LineageRole.installed,
  key: 'AAAA',
  name: 'Sam',
  version: version,
  released: released ?? [_h(3)],
  capabilities: const {},
  title: 'Plant log',
  updatesStopped: stopped,
  link: link,
);

/// A head at [version] with content [hash], whose latest bundle is at
/// public version [bundleAt] (null: none) with bundle version [bv].
RemoteHead _head(int version, String hash, {int? bundleAt, int? bv, String? bundleHash}) =>
    RemoteHead(
      artifactId: _raid,
      url: '$_origin/a/$_raid',
      title: 'Plant log',
      version: version,
      contentSha256: hash,
      publishedBy: _by,
      owner: _by,
      comments: false,
      bundle: bundleAt == null
          ? null
          : RemoteBundle(
              version: bundleAt,
              lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
              bundleVersion: bv ?? bundleAt,
              contentSha256: bundleHash ?? hash,
              url: '$_origin/api/artifacts/$_raid/versions/$bundleAt/bundle',
              publishedBy: _by,
            ),
    );

void main() {
  group('the §5.4 table', () {
    final author = _link(AppLink.author);
    final recipient = _link(AppLink.recipient);
    final rows =
        <(String, AppLink?, String, LineageEntry?, RemoteHead?, bool, SyncState)>[
          ('only here: no link', null, _h(3), _entry(), null, false, SyncState.onlyHere),
          ('live: the head is what this device published', author, _h(3),
              _entry(link: author), _head(3, _h(3), bundleAt: 3), false, SyncState.live),
          ('ahead: local versions past the base, head unchanged', author, _h(9),
              _entry(link: author), _head(3, _h(3), bundleAt: 3), true, SyncState.ahead),
          ('ahead: live differs from the unchanged head', author, _h(9),
              _entry(link: author), _head(3, _h(3), bundleAt: 3), false, SyncState.ahead),
          ('update waiting: a newer bundle head, local unmodified', author, _h(3),
              _entry(link: author), _head(4, _h(4), bundleAt: 4), false, SyncState.updateWaiting),
          ('update waiting: a web-made head over an unmoved author (Take theirs)',
              author, _h(3), _entry(link: author),
              _head(4, _h(4), bundleAt: 3, bundleHash: _h(3)), false,
              SyncState.updateWaiting),
          ('update waiting: an author at bundle 3, base 1, and a web-made v2 with '
              'no bundle', _link(AppLink.author, base: 1, taken: 3), _h(1),
              _entry(link: _link(AppLink.author, base: 1, taken: 3), released: [_h(1)]),
              _head(2, _h(2)), false, SyncState.updateWaiting),
          ('diverged: both moved', author, _h(9), _entry(link: author),
              _head(4, _h(4), bundleAt: 4), true, SyncState.diverged),
          ('diverged: a web-made head and local versions', author, _h(9),
              _entry(link: author), _head(4, _h(4), bundleAt: 3, bundleHash: _h(3)), true,
              SyncState.diverged),
          ('kept mine', recipient, _h(9), _entry(stopped: true, link: recipient),
              _head(4, _h(4), bundleAt: 4, bv: 4), true, SyncState.keptMine),
          ('kept mine, for an author too', author, _h(9),
              _entry(stopped: true, link: author), _head(4, _h(4), bundleAt: 4), true,
              SyncState.keptMine),
          ('a stale head older than the base is not ↓', author, _h(3),
              _entry(link: author), _head(2, _h(2), bundleAt: 2), false, SyncState.live),
          ('no head: what is known locally', author, _h(9), _entry(link: author), null,
              true, SyncState.ahead),
          ('no head, unmodified: live', author, _h(3), _entry(link: author), null, false,
              SyncState.live),
          ('recipient: a newer bundle head is ↓', recipient, _h(3),
              _entry(link: recipient), _head(4, _h(4), bundleAt: 4, bv: 4), false,
              SyncState.updateWaiting),
          ('recipient: modified with a newer bundle is ↓, never diverged', recipient,
              _h(9), _entry(link: recipient), _head(4, _h(4), bundleAt: 4, bv: 4), true,
              SyncState.updateWaiting),
          ('recipient: modified, nothing newer, is a fork (⑂), never ahead',
              recipient, _h(9), _entry(link: recipient),
              _head(3, _h(3), bundleAt: 3), true, SyncState.forked),
          ('recipient: a fork offline is still a fork', recipient, _h(9),
              _entry(link: recipient), null, true, SyncState.forked),
          ('recipient: reset to the release it took is live again', recipient,
              _h(3), _entry(link: recipient), _head(3, _h(3), bundleAt: 3), false,
              SyncState.live),
          ('recipient: merged in (live is what Bring vN into mine published) is '
              'live again', _link(AppLink.recipient, merged: _h(9)), _h(9),
              _entry(link: _link(AppLink.recipient, merged: _h(9))),
              _head(3, _h(3), bundleAt: 3), true, SyncState.live),
          ('recipient: changed again after a merge is a fork', _link(
                  AppLink.recipient, merged: _h(9)), _h(10),
              _entry(link: _link(AppLink.recipient, merged: _h(9))),
              _head(3, _h(3), bundleAt: 3), true, SyncState.forked),
          ('recipient: a web-made head is invisible', recipient, _h(3),
              _entry(link: recipient), _head(4, _h(4), bundleAt: 3, bundleHash: _h(3)),
              false, SyncState.live),
          ('recipient: a web-made head over a bundle at the base is not ↓, '
              'whatever bundle version is held', recipient, _h(1),
              _entry(version: 1, released: [_h(1)], link: recipient),
              _head(4, _h(4), bundleAt: 3, bv: 3), false, SyncState.live),
          ('recipient: a bundle at a version held already is not ↓', recipient, _h(3),
              _entry(version: 5, link: _link(AppLink.recipient, taken: 5)),
              _head(4, _h(4), bundleAt: 4, bv: 5), false, SyncState.live),
        ];
    for (final (name, link, live, entry, head, modified, want) in rows) {
      test(name, () {
        expect(
          syncState(
            link: link,
            liveContentSha256: live,
            entry: entry,
            head: head,
            modified: modified,
          ),
          want,
        );
      });
    }
  });

  test('a link keeps its merged hash through JSON', () {
    final l = _link(AppLink.recipient, taken: 4, merged: _h(9));
    expect(AppLink.fromJson(l.toJson())!.mergedSha256, _h(9));
    expect(AppLink.fromJson(_link(AppLink.recipient).toJson())!.mergedSha256,
        isNull);
  });

  test('over random histories: never ahead for a recipient, diverged only '
      'when both moved, never ↓ for a head at or below what is held', () {
    final reached = <(String, SyncState)>{};
    for (var seed = 0; seed < 400; seed++) {
      final w = _World(
        Random(seed),
        seed.isEven ? AppLink.author : AppLink.recipient,
      );
      for (var step = 0; step < 30; step++) {
        w.act();
        final (state, passed) = w.observe();
        reached.add((w.role, state));
        w.check(state, passed, 'seed $seed step $step: ${w.log.join(', ')}');
      }
    }
    // The generator reaches every state the role can be in, so each guard
    // in syncState is exercised.
    expect(reached, containsAll(<(String, SyncState)>{
      (AppLink.author, SyncState.live),
      (AppLink.author, SyncState.ahead),
      (AppLink.author, SyncState.updateWaiting),
      (AppLink.author, SyncState.diverged),
      (AppLink.author, SyncState.keptMine),
      (AppLink.recipient, SyncState.live),
      (AppLink.recipient, SyncState.forked),
      (AppLink.recipient, SyncState.updateWaiting),
      (AppLink.recipient, SyncState.keptMine),
    }));
  });
}

/// A public version as the model keeps it.
class _Pub {
  _Pub(this.version, this.hash, this.bundleAt, this.bundleVersion, this.bundleHash);

  final int version;
  final String hash;
  final int? bundleAt;
  final int? bundleVersion;
  final String? bundleHash;

  RemoteHead get head =>
      _head(version, hash, bundleAt: bundleAt, bv: bundleVersion, bundleHash: bundleHash);
}

/// One local app linked to a public copy, and everyone else publishing
/// there, as plain state the actions move.
class _World {
  _World(this.r, this.role) {
    heads.add(_Pub(1, _h(1), 1, 1, _h(1)));
  }

  final Random r;
  final String role;
  final log = <String>[];
  var _next = 100;

  /// Every head the public copy has had, the current one last.
  final heads = <_Pub>[];
  var live = _h(1);
  var released = [_h(1)];
  var bundleVersion = 1;
  var base = 1;
  var stopped = false;

  _Pub get current => heads.last;
  bool get author => role == AppLink.author;
  int get maxBundle => heads.map((h) => h.bundleVersion ?? 0).reduce(max);

  String fresh() => _h(_next++);

  void act() {
    switch (r.nextInt(6)) {
      case 0: // publish
        log.add('publish');
        if (!author || current.version > base) return;
        final n = current.version + 1;
        final bv = max(maxBundle, bundleVersion) + 1;
        heads.add(_Pub(n, live, n, bv, live));
        base = n;
        bundleVersion = bv;
        released = [...released.where((h) => h != live), live];
      case 1: // editLocal
        log.add('edit');
        live = fresh();
      case 2: // remotePublishBundle
        log.add('remote bundle');
        final n = current.version + 1;
        final h = fresh();
        heads.add(_Pub(n, h, n, maxBundle + 1, h));
      case 3: // remotePublishWeb
        log.add('remote web');
        heads.add(
          _Pub(
            current.version + 1,
            fresh(),
            current.bundleAt,
            current.bundleVersion,
            current.bundleHash,
          ),
        );
      case 4: // keepMine, offered on a modified copy
        log.add('keep mine');
        if (modified) stopped = true;
      case 5: // update: take theirs
        log.add('update');
        final c = current;
        if (author) {
          if (c.version <= base) return;
          live = c.hash;
          base = c.version;
          if (c.bundleAt == c.version) bundleVersion = c.bundleVersion!;
          released = [...released.where((h) => h != live), live];
        } else {
          if (c.bundleAt == null ||
              c.bundleAt! <= base ||
              c.bundleVersion! <= bundleVersion) {
            return;
          }
          live = c.bundleHash!;
          base = c.bundleAt!;
          bundleVersion = c.bundleVersion!;
          released = [...released.where((h) => h != live), live];
        }
        stopped = false;
    }
  }

  /// Author: the live version is not the one last published or taken.
  /// Recipient: §5.4's modified copy.
  bool get modified =>
      author ? live != released.last : !released.contains(live);

  /// The state for the head the checker last read: the current one, none
  /// (offline), or one cached from earlier.
  (SyncState, RemoteHead?) observe() {
    final pick = r.nextInt(10);
    final passed = pick < 7
        ? current.head
        : pick < 8
        ? null
        : heads[r.nextInt(heads.length)].head;
    final link = _link(role, base: base, taken: author ? null : bundleVersion);
    final state = syncState(
      link: link,
      liveContentSha256: live,
      entry: _entry(
        version: bundleVersion,
        released: released,
        stopped: stopped,
        link: link,
      ),
      head: passed,
      modified: modified,
    );
    return (state, passed);
  }

  void check(SyncState s, RemoteHead? head, String where) {
    if (stopped) {
      expect(s, SyncState.keptMine, reason: where);
      return;
    }
    if (!author) {
      expect(s, isNot(SyncState.ahead), reason: where);
      expect(s, isNot(SyncState.diverged), reason: where);
      final b = head?.bundle;
      final newer =
          b != null && b.version > base && b.bundleVersion > bundleVersion;
      expect(
        s,
        newer
            ? SyncState.updateWaiting
            : modified
            ? SyncState.forked
            : SyncState.live,
        reason: where,
      );
      return;
    }
    final remoteMoved = head != null && head.version > base;
    final localMoved = live != released.last;
    expect(
      s == SyncState.diverged,
      remoteMoved && localMoved,
      reason: 'diverged only when both moved, a web-made head included; $where',
    );
    if (s == SyncState.updateWaiting) {
      expect(head!.version, greaterThan(base), reason: 'never ↓ at or below the base; $where');
      expect(localMoved, isFalse, reason: 'never ↓ over local versions; $where');
    }
    expect(
      s == SyncState.ahead,
      localMoved && !remoteMoved,
      reason: 'ahead when only local moved; $where',
    );
  }
}
