/// Which remote a linked app's public copy is reached through: the
/// connected remote at the link's origin, found when it is used, since a
/// disconnect and a later sign-in give the same hosted Clax a new remote ID
/// in the core (spec 2026-10-21-app-push-pull §5.2).
library;

import '../bundle/lineage_store.dart';
import '../clax/api.dart' show ClaxApiError;
import 'models.dart';
import 'remote_api.dart';

/// [url]'s origin (`https://clax.dev`), as links and remotes compare
/// them.
String originOf(String url) => Uri.parse(url).origin;

/// The connected remote at [origin] among [remotes], or null.
Remote? connectedAt(Iterable<Remote> remotes, String origin) {
  final want = originOf(origin);
  for (final r in remotes) {
    if (r.connected && originOf(r.origin) == want) return r;
  }
  return null;
}

/// Resolves linked apps' remotes through [api], keeping [store]'s links
/// and the core's links in step with the remote signed in to now.
class LinkRemotes {
  LinkRemotes({
    required this.api,
    required this.store,
    void Function(String line)? log,
  }) : _log = log ?? ((_) {});

  final RemoteApi api;
  final LineageStore store;
  final void Function(String) _log;

  /// The connected remote at [origin], or null when there is none or the
  /// remotes cannot be read.
  Future<Remote?> at(String origin) async {
    try {
      return connectedAt(await api.remotes(), origin);
    } on ClaxApiError catch (e) {
      _log('the remotes could not be read: ${e.message}');
      return null;
    }
  }

  /// [aid]'s link, its [AppLink.remoteId] the connected remote at its
  /// origin, or null when this device is not signed in there (the stored
  /// ID is then left as it is). When that remote is not the one stored,
  /// the core's link is made again on it ([RemoteApi.link], based at the
  /// link's [AppLink.baseRemoteVersion]) and the new ID stored. Null
  /// without a link. Throws the [ClaxApiError] of a link the core refused,
  /// leaving the stored ID as it was.
  Future<AppLink?> resolve(String aid) async {
    final entry = await store.entry(aid);
    final link = entry?.link;
    if (entry == null || link == null) return null;
    final r = await at(link.remoteOrigin);
    if (r == null) return link.copyWith(remoteId: () => null);
    if (r.id == link.remoteId) return link;
    await api.link(
      r.id,
      aid,
      RemoteApp(
        artifactId: link.remoteArtifactId,
        title: entry.title,
        url: link.url,
        version: link.baseRemoteVersion,
        bundleVersion: link.takenBundleVersion ?? 0,
        access: link.isAuthor ? 'editor' : 'viewer',
        general: 'restricted',
      ),
      link.baseRemoteVersion,
    );
    await store.update(aid, (e) async {
      final l = e?.link;
      if (e == null || l == null || l.remoteArtifactId != link.remoteArtifactId) {
        return null;
      }
      return e.copyWith(link: () => l.copyWith(remoteId: () => r.id));
    });
    _log('$aid is linked again through ${r.name} (${r.id})');
    return link.copyWith(remoteId: () => r.id);
  }

  /// Links every app linked at [r]'s origin to [r] again (after a sign-in);
  /// one the core refuses is left for its next use.
  Future<void> relinkAll(Remote r) async {
    final want = originOf(r.origin);
    for (final MapEntry(key: aid, value: e) in (await store.all()).entries) {
      final l = e.link;
      if (l == null || originOf(l.remoteOrigin) != want || l.remoteId == r.id) {
        continue;
      }
      try {
        await resolve(aid);
      } on ClaxApiError catch (err) {
        _log('$aid could not be linked again: ${err.message}');
      }
    }
  }
}
