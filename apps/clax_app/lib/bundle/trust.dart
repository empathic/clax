import 'lineage_store.dart';
import 'manifest.dart';

/// What opening a verified bundle does here (§6.4).
enum TrustAnswer {
  /// This install signed it and has the lineage as an author app.
  own,

  /// A lineage new to this install.
  install,

  /// A higher version of an installed app, under the same key.
  update,

  /// The same version, content hash, title and capabilities.
  current,

  /// The same version, differing in one of those.
  conflict,

  /// A lower version.
  older,

  /// The lineage is a local app's under another key: a separate install.
  foreign,

  /// A local fork names the lineage and key upstream: a separate install.
  forked,
}

/// The answer of [decide] and the local app it concerns.
class TrustDecision {
  const TrustDecision(
    this.answer, {
    this.aid,
    this.entry,
    this.modified = false,
    this.stopped = false,
  });

  final TrustAnswer answer;

  /// The local app the answer is about; null for [TrustAnswer.install].
  final String? aid;

  /// [aid]'s lineage entry.
  final LineageEntry? entry;

  /// For [TrustAnswer.update]: the local copy is modified (§5.4).
  final bool modified;

  /// For [TrustAnswer.update]: the person chose "Keep mine" before.
  final bool stopped;
}

/// §6.4's trust decision for [bundle], over the lineage [store] (by
/// artifact ID) and this install's author key [ownKey] (null before the
/// first export). Pure.
///
/// [live] holds the live release state of local apps on the bundle's
/// lineage, by artifact ID, for §5.4's modified rule; an app missing from
/// it counts as modified, so the person is asked before their changes
/// could be replaced.
///
/// Rows are matched in this order: `own`; an app with the bundle's lineage
/// and key (`update`, `current`, `conflict`, `older`), so a separate
/// install takes its own lineage's later bundles; `forked`; `foreign`;
/// else `install`. Apps are visited in artifact ID order.
TrustDecision decide(
  Manifest bundle,
  Map<String, LineageEntry> store,
  String? ownKey, {
  Map<String, ReleaseState> live = const {},
}) {
  final key = bundle.author.key;
  final aids = store.keys.toList()..sort();
  Iterable<(String, LineageEntry)> where(bool Function(LineageEntry) test) => [
    for (final aid in aids)
      if (test(store[aid]!)) (aid, store[aid]!),
  ];

  if (ownKey != null && key == ownKey) {
    for (final (aid, e) in where(
      (e) =>
          e.role == LineageRole.author &&
          e.lineage == bundle.lineage &&
          e.key == key,
    )) {
      return TrustDecision(TrustAnswer.own, aid: aid, entry: e);
    }
  }

  for (final (aid, e) in where(
    (e) =>
        e.role == LineageRole.installed &&
        e.lineage == bundle.lineage &&
        e.key == key,
  )) {
    if (bundle.version > e.version) {
      final state = live[aid];
      return TrustDecision(
        TrustAnswer.update,
        aid: aid,
        entry: e,
        modified:
            state == null ||
            e.isModified(
              contentSha256: state.contentSha256,
              capabilities: state.capabilities,
            ),
        stopped: e.updatesStopped,
      );
    }
    if (bundle.version < e.version) {
      return TrustDecision(TrustAnswer.older, aid: aid, entry: e);
    }
    final same = e.isLatestRelease(
      ReleaseState(
        contentSha256: bundle.contentSha256,
        title: bundle.title,
        capabilities: bundle.capabilities,
      ),
    );
    return TrustDecision(
      same ? TrustAnswer.current : TrustAnswer.conflict,
      aid: aid,
      entry: e,
    );
  }

  for (final (aid, e) in where(
    (e) => e.upstream?.lineage == bundle.lineage && e.upstream?.key == key,
  )) {
    return TrustDecision(TrustAnswer.forked, aid: aid, entry: e);
  }

  for (final (aid, e) in where((e) => e.lineage == bundle.lineage)) {
    return TrustDecision(TrustAnswer.foreign, aid: aid, entry: e);
  }

  return const TrustDecision(TrustAnswer.install);
}
