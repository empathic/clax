import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/manifest.dart';
import 'package:clax_app/bundle/trust.dart';
import 'package:clax_app/remote/models.dart';
import 'package:flutter_test/flutter_test.dart';

const _own = 'OOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOO';
const _author = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
const _other = 'ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ';
const _lin = 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa';
const _fork = 'l_ffffffffffffffffffffffffff';
final _h1 = 'sha256:${'1' * 64}';
final _h2 = 'sha256:${'2' * 64}';
final _h3 = 'sha256:${'3' * 64}';
const _caps = <String, Object?>{
  'db': {'write': true},
};

Manifest _bundle({
  String key = _author,
  int version = 2,
  String? hash,
  String title = 'Plant log',
  Map<String, Object?> capabilities = _caps,
}) => Manifest(
  lineage: _lin,
  version: version,
  title: title,
  icon: const ManifestIcon(svg: null, word: 'leaf'),
  capabilities: capabilities,
  kit: null,
  files: {
    'index.html': ManifestFile(sha256: '0' * 64, size: 1, type: 'text/html'),
  },
  contentSha256: hash ?? _h2,
  seed: null,
  author: ManifestAuthor(key: key, name: 'Sam'),
  forkedFrom: null,
);

LineageEntry _entry({
  String lineage = _lin,
  LineageRole role = LineageRole.installed,
  String key = _author,
  int version = 1,
  List<String>? released,
  bool stopped = false,
  LineageUpstream? upstream,
  AppLink? link,
}) => LineageEntry(
  lineage: lineage,
  role: role,
  key: key,
  name: 'Sam',
  version: version,
  released: released ?? [_h1],
  capabilities: _caps,
  title: 'Plant log',
  updatesStopped: stopped,
  upstream: upstream,
  link: link,
);

const _remote = 'https://clax.example';
const _by = PublishedBy(name: 'Sam');

AppLink _link({String raid = 'ra1', String origin = _remote}) => AppLink(
  remoteOrigin: origin,
  remoteId: 'r1',
  remoteArtifactId: raid,
  url: '$origin/a/$raid',
  baseRemoteVersion: 1,
  role: AppLink.recipient,
);

/// [raid] on [_remote], fetched at public version 2.
BundleOrigin _from({String raid = 'ra1'}) => BundleOrigin(
  remoteOrigin: _remote,
  remoteArtifactId: raid,
  remoteVersion: 2,
  head: RemoteHead(
    artifactId: raid,
    url: '$_remote/a/$raid',
    title: 'Plant log',
    version: 2,
    contentSha256: _h2,
    publishedBy: _by,
    owner: _by,
    comments: false,
  ),
);

void main() {
  test('own: this install signed it and has the lineage as author', () {
    final d = decide(_bundle(key: _own), {
      'a1': _entry(role: LineageRole.author, key: _own),
    }, _own);
    expect(d.answer, TrustAnswer.own);
    expect(d.aid, 'a1');
  });

  test(
    'own without the keychain: an author entry with the lineage and key',
    () {
      final d = decide(_bundle(key: _own), {
        'a1': _entry(role: LineageRole.author, key: _own),
      }, null);
      expect(d.answer, TrustAnswer.own);
      expect(d.aid, 'a1');
      expect(d.mine, isTrue);
    },
  );

  test('own bundle after deleting the app: install, marked as mine', () {
    final d = decide(_bundle(key: _own), {}, _own);
    expect(d.answer, TrustAnswer.install);
    expect(d.mine, isTrue);
    expect(decide(_bundle(), {}, _own).mine, isFalse);
    expect(decide(_bundle(key: _own), {}, null).mine, isFalse);
  });

  test('install: no local app has the lineage or names it upstream', () {
    final store = {'x': _entry(lineage: _fork, key: _other)};
    expect(decide(_bundle(), store, _own).answer, TrustAnswer.install);
    expect(decide(_bundle(), {}, null).answer, TrustAnswer.install);
    // This install's key on a lineage it no longer has: a plain install.
    expect(decide(_bundle(key: _own), {}, _own).answer, TrustAnswer.install);
  });

  group('update', () {
    final store = {
      'a1': _entry(version: 1, released: [_h1]),
    };

    test('unmodified copy', () {
      final d = decide(
        _bundle(version: 2),
        store,
        _own,
        live: {
          'a1': ReleaseState(
            contentSha256: _h1,
            title: 'Plant log',
            capabilities: _caps,
          ),
        },
      );
      expect(d.answer, TrustAnswer.update);
      expect(d.aid, 'a1');
      expect(d.entry!.version, 1);
      expect(d.modified, isFalse);
      expect(d.stopped, isFalse);
    });

    test('modified copy', () {
      final d = decide(
        _bundle(version: 2),
        store,
        _own,
        live: {
          'a1': ReleaseState(
            contentSha256: _h3,
            title: 'Plant log',
            capabilities: _caps,
          ),
        },
      );
      expect(d.answer, TrustAnswer.update);
      expect(d.modified, isTrue);
    });

    test('an app whose live state is not given counts as modified', () {
      expect(decide(_bundle(version: 2), store, _own).modified, isTrue);
    });

    test('stopped updates', () {
      final d = decide(
        _bundle(version: 2),
        {'a1': _entry(stopped: true)},
        _own,
        live: {
          'a1': ReleaseState(
            contentSha256: _h3,
            title: 'Plant log',
            capabilities: _caps,
          ),
        },
      );
      expect(d.answer, TrustAnswer.update);
      expect(d.stopped, isTrue);
    });
  });

  group('same version', () {
    final store = {
      'a1': _entry(version: 2, released: [_h1, _h2]),
    };

    test('current: same hash, title and capabilities', () {
      final d = decide(_bundle(version: 2), store, _own);
      expect(d.answer, TrustAnswer.current);
      expect(d.aid, 'a1');
    });

    test('conflict: hash, title or capabilities differ', () {
      expect(
        decide(_bundle(version: 2, hash: _h3), store, _own).answer,
        TrustAnswer.conflict,
      );
      expect(
        decide(_bundle(version: 2, title: 'Plants'), store, _own).answer,
        TrustAnswer.conflict,
      );
      expect(
        decide(_bundle(version: 2, capabilities: const {}), store, _own).answer,
        TrustAnswer.conflict,
      );
      // An earlier release's hash under the latest version number differs.
      expect(
        decide(_bundle(version: 2, hash: _h1), store, _own).answer,
        TrustAnswer.conflict,
      );
    });
  });

  test('older: a lower version is refused', () {
    final d = decide(_bundle(version: 1, hash: _h1), {
      'a1': _entry(version: 2, released: [_h1, _h2]),
    }, _own);
    expect(d.answer, TrustAnswer.older);
    expect(d.entry!.version, 2);
  });

  test('foreign: the lineage is a local app under another key', () {
    final installed = decide(_bundle(), {'a1': _entry(key: _other)}, _own);
    expect(installed.answer, TrustAnswer.foreign);
    expect(installed.aid, 'a1');
    // Someone else's bundle on a lineage this install authors.
    final authored = decide(_bundle(), {
      'a1': _entry(role: LineageRole.author, key: _own),
    }, _own);
    expect(authored.answer, TrustAnswer.foreign);
  });

  test('forked: a local fork names the lineage and key upstream', () {
    final fork = _entry(
      lineage: _fork,
      role: LineageRole.author,
      key: _own,
      upstream: const LineageUpstream(lineage: _lin, key: _author),
    );
    final d = decide(_bundle(), {'f1': fork}, _own);
    expect(d.answer, TrustAnswer.forked);
    expect(d.aid, 'f1');
    // Only a local fork (role author) counts, not an installed entry
    // that happens to carry an upstream.
    final notFork = decide(_bundle(), {
      'i1': _entry(
        lineage: _fork,
        key: _other,
        upstream: const LineageUpstream(lineage: _lin, key: _author),
      ),
    }, _own);
    expect(notFork.answer, TrustAnswer.install);
    // A fork of the same lineage under another key does not match.
    final otherKey = decide(_bundle(key: _other), {'f1': fork}, _own);
    expect(otherKey.answer, TrustAnswer.install);
  });

  test('a separate install takes the lineage\'s later bundles as updates', () {
    // After "Install separately", the new app's entry has the bundle's
    // lineage and key: that app updates, not the fork or the foreign one.
    final store = {
      'f1': _entry(
        lineage: _fork,
        role: LineageRole.author,
        key: _own,
        upstream: const LineageUpstream(lineage: _lin, key: _author),
      ),
      'a0': _entry(key: _other),
      'a2': _entry(version: 1),
    };
    final d = decide(_bundle(version: 2), store, _own);
    expect(d.answer, TrustAnswer.update);
    expect(d.aid, 'a2');
  });

  group('linked', () {
    test('linked_update_from_other_key_is_update', () {
      final d = decide(
        _bundle(key: _other, version: 2),
        {'a1': _entry(link: _link())},
        _own,
        live: {
          'a1': ReleaseState(
            contentSha256: _h1,
            title: 'Plant log',
            capabilities: _caps,
          ),
        },
        origin: _from(),
      );
      expect(d.answer, TrustAnswer.update);
      expect(d.aid, 'a1');
      expect(d.linked, isTrue);
      expect(d.modified, isFalse);
    });

    test('linked_same_bundle_version_is_current', () {
      final d = decide(
        _bundle(key: _other, version: 1, hash: _h1),
        {'a1': _entry(link: _link())},
        _own,
        origin: _from(),
      );
      expect(d.answer, TrustAnswer.current);
      expect(d.linked, isTrue);
    });

    test('a linked lower version is older, whatever the key', () {
      final d = decide(
        _bundle(key: _other, version: 1),
        {'a1': _entry(version: 3, link: _link())},
        _own,
        origin: _from(),
      );
      expect(d.answer, TrustAnswer.older);
      expect(d.linked, isTrue);
    });

    test('the linked row comes before own', () {
      final d = decide(
        _bundle(key: _own, version: 4),
        {
          'a1': _entry(
            role: LineageRole.author,
            key: _own,
            version: 3,
            link: _link(),
          ),
        },
        _own,
        origin: _from(),
      );
      expect(d.answer, TrustAnswer.update);
      expect(d.linked, isTrue);
      expect(d.mine, isTrue);
    });

    test('unlinked_other_key_still_foreign', () {
      // The same lineage under another key, linked to another public copy
      // or to none, is the existing table's.
      for (final store in [
        {'a1': _entry()},
        {'a1': _entry(link: _link(raid: 'ra2'))},
        {'a1': _entry(link: _link(origin: 'https://other.example'))},
      ]) {
        final d = decide(
          _bundle(key: _other),
          store,
          _own,
          origin: _from(),
        );
        expect(d.answer, TrustAnswer.foreign);
        expect(d.linked, isFalse);
      }
      // Without an origin, a linked entry is no different.
      final d = decide(
        _bundle(key: _other),
        {'a1': _entry(link: _link())},
        _own,
      );
      expect(d.answer, TrustAnswer.foreign);
    });

    test('a linked entry of another lineage is not matched', () {
      final d = decide(
        _bundle(key: _other),
        {'a1': _entry(lineage: _fork, link: _link())},
        _own,
        origin: _from(),
      );
      expect(d.answer, TrustAnswer.install);
    });
  });
}
