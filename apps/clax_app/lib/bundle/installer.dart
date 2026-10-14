/// Opening a `.clax` file and installing it as a new app (bundles spec
/// §3.2, §6, §8.1).
library;

import 'dart:async';
import 'dart:typed_data';

import '../clax/api.dart';
import '../platform/bundle_key.dart';
import 'bundle_reader.dart';
import 'doc_snapshot.dart';
import 'lineage_store.dart';
import 'trust.dart';

/// A bundle service's failure, in one sentence for the sheet ([message]),
/// with [code] (a [BundleError] code, or the service's own) and the
/// longer [details] for the sheet's Details.
class BundleFailure implements Exception {
  const BundleFailure(this.code, this.message, {this.details = const []});

  final String code;
  final String message;
  final List<String> details;

  /// The sheet's sentence for a refused bundle.
  factory BundleFailure.fromError(BundleError e) => BundleFailure(
    e.code,
    switch (e.code) {
      'too_new' => 'Made by a newer Clax: update Clax to open it.',
      'too_large' => 'This file is too large to be a Clax app.',
      'not_a_bundle' => 'This file is not a Clax app.',
      'not_shareable' => 'This app reads other apps, which shared apps cannot.',
      _ => 'This file is damaged or was changed after it was signed.',
    },
    details: [e.message],
  );

  @override
  String toString() => 'BundleFailure($code): $message';
}

/// A verified bundle and what opening it here does: the [decision] that
/// picks the sheet, with what the sheet shows.
class OpenedBundle {
  const OpenedBundle({
    required this.bundle,
    required this.decision,
    required this.fingerprint,
    required this.fromSelf,
    this.live,
  });

  final Bundle bundle;
  final TrustDecision decision;

  /// The author key's fingerprint (`7KQ2-M9XD-4FTA-QW3B`).
  final String fingerprint;

  /// Whether this install's key signed it ("From you").
  final bool fromSelf;

  /// For [TrustAnswer.update]: the local app's live release state, to
  /// list what the update adds ([addedCapabilities]).
  final ReleaseState? live;

  Manifest get manifest => bundle.manifest;

  /// The author's name or the fingerprint: who the label and note name.
  String get who => manifest.author.name ?? fingerprint;
}

/// Opens and installs bundles.
class Installer {
  Installer({required this.api, required this.store, required this.key});

  final ClaxApi api;
  final LineageStore store;
  final BundleKey key;

  /// Reads and verifies [bytes] (§6.2) and decides what opening it does
  /// (§6.4), with the live release state of each local app on its lineage.
  /// A local app the daemon no longer has is forgotten first. Throws
  /// [BundleFailure] for a refused file.
  Future<OpenedBundle> open(Uint8List bytes) async {
    final Bundle bundle;
    try {
      bundle = await BundleReader.read(bytes);
    } on BundleError catch (e) {
      throw BundleFailure.fromError(e);
    }
    final m = bundle.manifest;
    var entries = await store.all();
    final live = <String, ReleaseState>{};
    for (final MapEntry(key: aid, value: e) in entries.entries) {
      if (e.lineage != m.lineage && e.upstream?.lineage != m.lineage) {
        continue;
      }
      final state = await liveRelease(api, aid);
      if (state == null) {
        await store.forget(aid);
      } else {
        live[aid] = state;
      }
    }
    entries = await store.all();
    String? own;
    try {
      own = (await key.existing())?.key;
    } on Object {
      own = null;
    }
    final decision = decide(m, entries, own, live: live);
    return OpenedBundle(
      bundle: bundle,
      decision: decision,
      fingerprint: keyFingerprint(m.author.key) ?? m.author.key,
      fromSelf: own != null && own == m.author.key,
      live: decision.aid == null ? null : live[decision.aid],
    );
  }

  /// Installs [opened] as a new app (§8.1): for [TrustAnswer.install],
  /// [TrustAnswer.foreign] and [TrustAnswer.forked]. Returns its artifact
  /// ID. A bundle of this install's own ([TrustDecision.mine], on
  /// `install`: the person's app deleted here and opened again) is
  /// recorded with role `author`, so later exports continue its lineage. A failure deletes the app made and throws [BundleFailure].
  Future<String> install(OpenedBundle opened) async {
    final m = opened.manifest;
    final b = opened.bundle;
    final who = opened.who;
    final PublishResult made;
    try {
      made = await api.createArtifact(
        PublishRequest(
          title: m.title,
          icon: m.icon.word,
          capabilities: m.capabilities,
          label: 'v${m.version} from $who',
          note: 'Installed v${m.version} of ${m.title} from $who',
          entries: {
            for (final MapEntry(:key, :value) in m.files.entries)
              key: FileEntry.bytes(b.files[key]!, type: value.type),
          },
        ),
      );
    } on ClaxApiError catch (e) {
      if (e.code == 'invalid_capabilities') {
        throw BundleFailure(
          'invalid_capabilities',
          'This app needs a newer Clax.',
          details: [e.message],
        );
      }
      throw BundleFailure(
        'install',
        'The app could not be installed.',
        details: [e.message],
      );
    }
    final aid = made.artifact.id;
    final mine =
        opened.decision.mine && opened.decision.answer == TrustAnswer.install;
    try {
      final got = made.version.contentSha256;
      if (got != m.contentSha256) {
        throw BundleFailure(
          'bad_file',
          'The app could not be installed: its files changed on the way in.',
          details: [
            'The installed version is $got, the bundle ${m.contentSha256}.',
          ],
        );
      }
      final seed = b.seed ?? const [];
      for (var i = 0; i < seed.length; i += docBatchLimit) {
        await api.batchDocs(aid, [
          for (final d in seed.skip(i).take(docBatchLimit))
            setWrite(d.path, d.data),
        ]);
      }
      await store.put(
        aid,
        LineageEntry(
          lineage: m.lineage,
          role: mine ? LineageRole.author : LineageRole.installed,
          key: m.author.key,
          name: m.author.name,
          version: m.version,
          released: [m.contentSha256],
          capabilities: m.capabilities,
          title: m.title,
          fromBundle: !mine,
        ),
      );
      if (!mine) await store.keepBundle(aid, b.bytes);
      return aid;
    } on Object catch (e) {
      try {
        await api.deleteArtifact(aid);
      } on ClaxApiError {
        // Already gone.
      }
      await store.forget(aid);
      if (e is BundleFailure) rethrow;
      throw BundleFailure(
        'install',
        'The app could not be installed.',
        details: ['$e'],
      );
    }
  }
}

/// [aid]'s live release state (its current version's content hash, its
/// title and capabilities as exported), or null when the daemon has no
/// such app.
Future<ReleaseState?> liveRelease(ClaxApi api, String aid) async {
  final ArtifactDetail d;
  try {
    d = await api.getArtifact(aid);
  } on ClaxApiError catch (e) {
    if (e.isNotFound) return null;
    rethrow;
  }
  final v = d.current;
  return ReleaseState(
    contentSha256: v?.contentSha256 ?? '',
    title: d.artifact.title,
    capabilities: shareableCapabilities(d.artifact.capabilities),
  );
}

/// [caps] as a bundle carries them (§4.4): `db.read` and `db.share`
/// removed, the `db` object keeping its other members.
Map<String, Object?> shareableCapabilities(Map<String, Object?> caps) => {
  for (final MapEntry(:key, :value) in caps.entries)
    key: key == 'db' && value is Map
        ? {
            for (final MapEntry(key: k, value: v) in value.entries)
              if (k != 'read' && k != 'share') k as String: v,
          }
        : value,
};

/// Whether [aid]'s code came from a bundle (its lineage entry's
/// `from_bundle`): the agent's T2 isolation (§9) for that app.
Future<bool> Function(String aid) fromBundleOf(LineageStore store) =>
    (aid) async => (await store.entry(aid))?.fromBundle ?? false;

/// Forgets each deleted app's lineage entry, kept bundle and any document
/// snapshot (§5.3) as `artifact_deleted` events arrive on [events].
/// Cancel the subscription to stop.
StreamSubscription<ClaxEvent> forgetDeleted(
  Stream<ClaxEvent> events,
  LineageStore store, {
  DocSnapshots? snapshots,
}) => events.where((e) => e.type == 'artifact_deleted').listen((e) async {
  final aid = e.data['artifact_id'];
  if (aid is! String) return;
  await store.forget(aid);
  snapshots?.discard(aid);
});
