import '../remote/models.dart' show BundleOrigin;
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
    this.mine = false,
    this.linked = false,
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

  /// The bundle is signed with this install's key: an author entry of
  /// its lineage and key matched ([TrustAnswer.own]), or the given own key
  /// equals the bundle's. On [TrustAnswer.install] (the person's own app,
  /// deleted here and opened again) the installer should record the
  /// lineage with role `author`, so later exports continue it rather than
  /// fork it.
  final bool mine;

  /// The answer came from the `linked` row: the bundle came from the public
  /// copy [entry]'s link names, so its version was weighed whatever key
  /// signed it ([TrustAnswer.update], [TrustAnswer.current],
  /// [TrustAnswer.conflict] or [TrustAnswer.older]).
  final bool linked;
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
/// [origin] names the public copy the bundle was fetched from, when it was.
/// The `linked` row comes before every other: an app whose lineage entry
/// has the bundle's lineage and a link to [origin]'s public copy weighs
/// the bundle's version against its own whatever key signed it, as
/// `update`, `current`, `conflict` or `older` ([TrustDecision.linked]).
/// Who may publish there is the remote's decision, made when it took the
/// push.
///
/// Then rows are matched in this order: `own` (an author entry with the
/// bundle's lineage and key; [ownKey] is not needed for it); an app with the bundle's lineage
/// and key (`update`, `current`, `conflict`, `older`), so a separate
/// install takes its own lineage's later bundles; `forked` (a local
/// author entry naming the lineage and key upstream); `foreign`;
/// else `install`. Apps are visited in artifact ID order.
TrustDecision decide(
  Manifest bundle,
  Map<String, LineageEntry> store,
  String? ownKey, {
  Map<String, ReleaseState> live = const {},
  BundleOrigin? origin,
}) {
  final key = bundle.author.key;
  final aids = store.keys.toList()..sort();
  Iterable<(String, LineageEntry)> where(bool Function(LineageEntry) test) => [
    for (final aid in aids)
      if (test(store[aid]!)) (aid, store[aid]!),
  ];

  final mine = ownKey != null && key == ownKey;

  if (origin != null) {
    for (final (aid, e) in where(
      (e) => e.lineage == bundle.lineage && origin.names(e.link),
    )) {
      return _weigh(bundle, aid, e, live, mine: mine, linked: true);
    }
  }

  // An author entry under the bundle's key exists only because this
  // install signed with that key, so it needs no keychain read.
  for (final (aid, e) in where(
    (e) =>
        e.role == LineageRole.author &&
        e.lineage == bundle.lineage &&
        e.key == key,
  )) {
    return TrustDecision(TrustAnswer.own, aid: aid, entry: e, mine: true);
  }

  for (final (aid, e) in where(
    (e) =>
        e.role == LineageRole.installed &&
        e.lineage == bundle.lineage &&
        e.key == key,
  )) {
    return _weigh(bundle, aid, e, live, mine: mine);
  }

  for (final (aid, e) in where(
    (e) =>
        e.role == LineageRole.author &&
        e.upstream?.lineage == bundle.lineage &&
        e.upstream?.key == key,
  )) {
    return TrustDecision(TrustAnswer.forked, aid: aid, entry: e, mine: mine);
  }

  for (final (aid, e) in where((e) => e.lineage == bundle.lineage)) {
    return TrustDecision(TrustAnswer.foreign, aid: aid, entry: e, mine: mine);
  }

  return TrustDecision(TrustAnswer.install, mine: mine);
}

/// [bundle] against app [aid]'s entry [e] of its lineage, by version:
/// `update` when higher, `older` when lower, else `current` when it is
/// the latest release here and `conflict` when not.
TrustDecision _weigh(
  Manifest bundle,
  String aid,
  LineageEntry e,
  Map<String, ReleaseState> live, {
  required bool mine,
  bool linked = false,
}) {
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
      mine: mine,
      linked: linked,
    );
  }
  if (bundle.version < e.version) {
    return TrustDecision(
      TrustAnswer.older,
      aid: aid,
      entry: e,
      mine: mine,
      linked: linked,
    );
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
    mine: mine,
    linked: linked,
  );
}
