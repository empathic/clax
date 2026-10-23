/// A linked app's sync state with its public copy.
library;

import 'dart:math' show max;

import '../bundle/lineage_store.dart';
import 'models.dart';

/// Where a local app stands against its public copy; exactly one holds.
enum SyncState {
  /// Not linked: no public copy.
  onlyHere,

  /// The live local version is the public head's (☁).
  live,

  /// This device may publish, has local versions past the shared base, and
  /// the public copy has not moved since (↑).
  ahead,

  /// The public copy moved past what this device last took, and taking it
  /// replaces nothing of this device's (↓); for a recipient, also a
  /// modified copy's update offer.
  updateWaiting,

  /// The person chose Keep mine.
  keptMine,

  /// Both moved: local versions past the shared base and a newer public
  /// head (⑂).
  diverged,

  /// A recipient's copy with changes of its own past the release it last
  /// took (a modified copy): functionally a fork of the public app until it
  /// is reset to a release or an update is brought into it (⑂).
  forked,
}

/// The sync state of a local app with [link] (null: not linked), live
/// content hash [liveContentSha256], lineage [entry], the public [head]
/// last read (null when none could be read) and [modified]: for an author,
/// whether the live version is past the one last published or taken; for
/// a recipient, whether the copy is modified (the contract's "Updating").
/// Pure.
///
/// - Keep mine ([LineageEntry.updatesStopped]) is [SyncState.keptMine].
/// - A recipient is never [SyncState.ahead] nor [SyncState.diverged]: a
///   head whose latest bundle is past both the link's base and the bundle
///   version held here is [SyncState.updateWaiting], modified or not (a
///   modified copy's update); otherwise a modified copy is
///   [SyncState.forked], unless its live version is the one Bring vN into
///   mine published ([AppLink.mergedSha256]), and anything else is
///   [SyncState.live]. A head made on the web, with no bundle of its own,
///   is invisible to it.
/// - For an author the public copy moved when [head] is past the link's
///   base, a web-made head included; the local app moved when [modified],
///   or when the live version is not the unmoved head's. Both moved is
///   [SyncState.diverged], only the public copy
///   [SyncState.updateWaiting], only the local app [SyncState.ahead],
///   neither [SyncState.live]. Over a moved head made on the web (its
///   version carries no bundle) an unmoved author is
///   [SyncState.updateWaiting] too: the update it is offered is Take
///   theirs, which reads the head's files (§5.10).
///
/// A head at or below what this device holds, a stale one cached from
/// earlier included, never moves anything, and no head reads as unmoved.
SyncState syncState({
  required AppLink? link,
  required String liveContentSha256,
  required LineageEntry? entry,
  required RemoteHead? head,
  required bool modified,
}) {
  if (link == null) return SyncState.onlyHere;
  if (entry?.updatesStopped ?? false) return SyncState.keptMine;
  if (!link.isAuthor) {
    final b = head?.bundle;
    final held = [?link.takenBundleVersion, ?entry?.version].fold<int>(0, max);
    final newer =
        b != null &&
        b.version > link.baseRemoteVersion &&
        b.bundleVersion > held;
    if (newer) return SyncState.updateWaiting;
    final merged = link.mergedSha256 == liveContentSha256;
    return modified && !merged ? SyncState.forked : SyncState.live;
  }
  final remoteMoved = head != null && head.version > link.baseRemoteVersion;
  final localMoved =
      modified ||
      (head != null &&
          head.version == link.baseRemoteVersion &&
          head.contentSha256 != liveContentSha256);
  return switch ((remoteMoved, localMoved)) {
    (true, true) => SyncState.diverged,
    (true, false) => SyncState.updateWaiting,
    (false, true) => SyncState.ahead,
    (false, false) => SyncState.live,
  };
}

/// The tile badge state [s] shows, as the home contract names it (spec
/// 2026-10-21-app-push-pull §5.4): `live` (☁), `ahead` (↑), `update` (↓,
/// the only warm-coloured one), `diverged` (⑂) or `fork` (⑂, a
/// recipient's changed copy); null for none (only here, kept mine).
String? syncBadge(SyncState s) => switch (s) {
  SyncState.live => 'live',
  SyncState.ahead => 'ahead',
  SyncState.updateWaiting => 'update',
  SyncState.diverged => 'diverged',
  SyncState.forked => 'fork',
  SyncState.onlyHere || SyncState.keptMine => null,
};
