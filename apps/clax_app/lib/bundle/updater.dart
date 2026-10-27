/// Updating an installed app from a later bundle of its lineage: checked
/// on a draft with a copy of the person's data, published only on a pass
/// (bundles spec §3.3, §3.4, §8.2–8.4).
library;

import 'dart:convert';
import 'dart:io';
import 'dart:math' show max;

import 'package:flutter/foundation.dart' show ValueNotifier;

import '../agent/inspector.dart';
import '../agent/tools/render_tool.dart' show perTestLimit, testLimit;
import '../clax/api.dart';
import '../platform/bundle_key.dart' show keyFingerprint;
import '../remote/merge.dart' show Merged;
import '../remote/models.dart'
    show AppLink, BundleOrigin, RemoteApp, VersionSource;
import 'bundle_reader.dart';
import 'content_hash.dart';
import 'doc_snapshot.dart';
import 'installer.dart' show PublishedFiles, approveCapabilities;
import 'lineage_store.dart';

/// What an update's capabilities add over the live app's (§8.2): new
/// top-level capabilities ([capabilities]) and new `net` hosts ([hosts]).
class CapabilityDelta {
  const CapabilityDelta({required this.capabilities, required this.hosts});

  /// New top-level capability names (`net` among them only when the live
  /// app declared none).
  final List<String> capabilities;

  /// `net` hosts the live app did not declare.
  final List<String> hosts;

  bool get isEmpty => capabilities.isEmpty && hosts.isEmpty;
}

/// What [next] declares beyond [live].
CapabilityDelta addedCapabilities(
  Map<String, Object?> live,
  Map<String, Object?> next,
) {
  List<String> hostsOf(Map<String, Object?> c) => switch (c['net']) {
    {'hosts': final List<Object?> h} => [
      for (final x in h) ?(x is String ? x : null),
    ],
    _ => const [],
  };
  final had = hostsOf(live).toSet();
  return CapabilityDelta(
    capabilities: [
      for (final k in next.keys)
        if (!live.containsKey(k)) k,
    ],
    hosts: [
      for (final h in hostsOf(next))
        if (!had.contains(h)) h,
    ],
  );
}

/// A step of an update, for the progress sheet's checking line (§3.3).
enum UpdateStep {
  /// Making the draft with a copy of the person's documents.
  data,

  /// Running `migrate.js` on the copy.
  migrate,

  /// Loading the page.
  page,

  /// Running the update's tests.
  tests,

  /// Publishing the update.
  publish,

  /// Running `migrate.js` on the live app.
  liveMigrate,
}

/// How an update ended.
sealed class UpdateOutcome {
  const UpdateOutcome();
}

/// The update is live as lineage version [version]; [truncated] when its
/// check ran on part of the person's data.
class UpdateLanded extends UpdateOutcome {
  const UpdateLanded(this.version, {this.truncated = false, this.notLinked});

  final int version;
  final bool truncated;

  /// The core's refusal to move the app's link to the public version
  /// taken, when it refused: the update is live, but the link's base
  /// stays where it was, here and in the core.
  final String? notLinked;
}

/// The check failed at [step], for [reason], the end of §3.4's sentence;
/// [details] are the failing step's messages. Nothing was published.
class UpdateCheckFailed extends UpdateOutcome {
  const UpdateCheckFailed(
    this.step,
    this.reason, {
    this.details = const [],
    this.truncated = false,
  });

  final UpdateStep step;
  final String reason;
  final List<String> details;
  final bool truncated;
}

/// Why a draft did not pass its check ([Updater.checkDraft]): the failing
/// [step], its [reason] (the end of §3.4's sentence) and the step's
/// [details].
class DraftCheckFailure {
  const DraftCheckFailure(this.step, this.reason, [this.details = const []]);

  final UpdateStep step;
  final String reason;
  final List<String> details;
}

/// The update did not run, or did not land, for [message] (a sentence for
/// the sheet): a change in progress, the app changed during the check,
/// the daemon refused. Nothing was published.
class UpdateRefused extends UpdateOutcome {
  const UpdateRefused(this.message, {this.details = const []});

  final String message;
  final List<String> details;
}

/// `migrate.js` failed on the live app: the documents and the version
/// were put back, on lineage version [backOn] (§8.2 step 7). [restored] is false when putting them back failed too; the
/// snapshot is then kept for [Updater.recoverInterrupted].
class UpdateRolledBack extends UpdateOutcome {
  const UpdateRolledBack(
    this.backOn, {
    this.details = const [],
    this.restored = true,
  });

  final int backOn;
  final List<String> details;
  final bool restored;
}

/// The person cancelled the update during its check, or did not take
/// it when asked: the draft is gone and nothing was published.
class UpdateCancelled extends UpdateOutcome {
  const UpdateCancelled();
}

/// Files taken without a release ([Updater.takeFiles]) are live as the
/// app's version [version], whose files hash to [contentSha256].
class FilesTaken extends UpdateOutcome {
  const FilesTaken(this.version, this.contentSha256);

  final int version;
  final String contentSha256;
}

/// An update put back, or waiting for the person, at start (§8.4).
///
/// [restored]: it was interrupted during its live migration and is back
/// on lineage version [backOn] with the documents as they were, for the
/// one-time notice. Not [restored]: its snapshot is kept because
/// documents may have been written since (a restore failed before, or the
/// app moved on); the person chooses [Updater.restoreKept] ("Restore") or
/// [Updater.discardKept] ("Discard").
class RecoveredUpdate {
  const RecoveredUpdate({
    required this.aid,
    required this.title,
    required this.backOn,
    this.restored = true,
  });

  final String aid;
  final String title;
  final int backOn;
  final bool restored;
}

/// Runs a body with the agent held off app `aid` (`FeedbackWorker.hold`).
typedef AppHold = Future<T> Function<T>(String aid, Future<T> Function() body);

Future<T> _noHold<T>(String aid, Future<T> Function() body) => body();

/// Updates installed apps from bundles.
///
/// While an update runs, the app is in [updating] (its screen shows an
/// "Updating…" cover and runs no page) and [hold] keeps the agent off it.
/// Before the publish, the release about to land is written to
/// `<support>/updates/<aid>.json` (with the bundle beside it), so a crash
/// between the publish and the lineage write is finished at the next
/// [recoverInterrupted].
class Updater {
  Updater({
    required this.api,
    required this.store,
    required this.inspector,
    required this.baseUrl,
    required this.snapshots,
    bool Function(String aid)? busy,
    AppHold? hold,
    this._ensureSession,
    this.migrationLimit = const Duration(seconds: 30),
    this.relink,
    void Function(String line)? log,
  }) : _busy = busy ?? ((_) => false),
       _hold = hold ?? _noHold,
       _log = log ?? ((_) {});

  final ClaxApi api;
  final LineageStore store;
  final Inspector inspector;

  /// Moves an app's link in the core to a public version taken
  /// (`POST /api/remotes/<rid>/links`, `RemoteApi.link`), throwing its refusal;
  /// null where nothing links apps. An update taken from the linked copy
  /// moves the link's base here only once the core's moved too, so the
  /// next publish's `if_version` is the version taken.
  Future<void> Function(String rid, String aid, RemoteApp app, int n)? relink;

  /// The shell's origin, as the render tool takes it.
  final Uri Function() baseUrl;
  final DocSnapshots snapshots;

  /// The longest a `migrate.js` run may take.
  final Duration migrationLimit;
  final bool Function(String) _busy;
  final AppHold _hold;
  final Future<void> Function()? _ensureSession;
  final void Function(String) _log;

  /// The apps an update is running on: their pages must not run.
  final updating = ValueNotifier<Set<String>>(const {});

  Directory get _pendingDir => Directory('${store.support.path}/updates');
  File _pending(String aid) => File('${_pendingDir.path}/$aid.json');
  File _pendingBundle(String aid) => File('${_pendingDir.path}/$aid.clax');

  /// Marks [aid]'s lineage `updates: stopped` ("Keep mine").
  Future<void> keepMine(String aid) => store.stopUpdates(aid);

  /// Updates app [aid] to [bundle] (§8.2), telling [onStep] each step.
  /// Never throws: a failed check, a refusal or an error is the outcome.
  ///
  /// When [cancelled] completes before the publish step, the check stops
  /// (each inspector run is told), its draft is deleted and the outcome is
  /// [UpdateCancelled]; from [UpdateStep.publish] on, the update finishes.
  ///
  /// [origin] names the public copy the bundle was fetched from: when the
  /// app's lineage entry links to it, a bundle of its lineage at a higher
  /// version is taken whatever key signed it (the `linked` row of
  /// `decide` in `trust.dart`).
  ///
  /// With [merged] (Bring vN into mine, spec 2026-10-21-app-push-pull
  /// §5.7) the update publishes the combined files, title and
  /// capabilities in place of the bundle's, checked and migrated the same
  /// way, and records the bundle's release as taken without counting the
  /// combined files as a release: the copy stays modified, now on vN.
  /// [confirm], when given, is asked once the check passed and before
  /// anything is published; false ends the update [UpdateCancelled].
  Future<UpdateOutcome> update(
    String aid,
    Bundle bundle, {
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
    BundleOrigin? origin,
    Merged? merged,
    Future<bool> Function()? confirm,
  }) async {
    try {
      if (_busy(aid)) {
        return const UpdateRefused('Wait for the change in progress to land.');
      }
      if (snapshots.has(aid) || _pending(aid).existsSync()) {
        return const UpdateRefused(
          'An earlier update of this app has not finished putting your data '
          'back. Restore or discard it first.',
        );
      }
      return await _hold(aid, () async {
        updating.value = {...updating.value, aid};
        try {
          return await _update(
            aid,
            bundle,
            onStep,
            cancelled,
            origin,
            merged,
            confirm,
          );
        } finally {
          updating.value = {...updating.value}..remove(aid);
        }
      });
    } on Object catch (e) {
      _log('the update of $aid failed: $e');
      return UpdateRefused('The update could not finish.', details: ['$e']);
    }
  }

  /// Why [entry] cannot take [m] as an update, or null when it can: the
  /// same lineage, at a lower version, and either installed under the same
  /// key or linked to [origin]'s public copy (whatever its role and key).
  static String? _mismatch(
    LineageEntry? entry,
    Manifest m,
    BundleOrigin? origin,
  ) {
    final linked = entry != null && (origin?.names(entry.link) ?? false);
    if (entry == null || (!linked && entry.role != LineageRole.installed)) {
      return 'This app was not installed from a bundle.';
    }
    if (entry.lineage != m.lineage || (!linked && entry.key != m.author.key)) {
      return 'This file is not an update of this app.';
    }
    if (m.version <= entry.version) {
      return 'This app is at v${entry.version} already.';
    }
    return null;
  }

  Future<UpdateOutcome> _update(
    String aid,
    Bundle bundle,
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
    BundleOrigin? origin,
    Merged? merged,
    Future<bool> Function()? confirm,
  ) async {
    var stop = false;
    cancelled?.then((_) => stop = true, onError: (Object _) => stop = true);
    final m = bundle.manifest;
    final entry = await store.entry(aid);
    if (_mismatch(entry, m, origin) case final why?) return UpdateRefused(why);
    final from = entry!.version;
    final ArtifactDetail live;
    try {
      live = await api.getArtifact(aid);
    } on ClaxApiError catch (e) {
      return UpdateRefused('The app could not be read.', details: [e.message]);
    }
    final base = live.artifact.currentVersion;
    final files = PublishedFiles.of(bundle);
    // Never clax.json: `capabilities` declares, and the daemon writes it.
    final entries = merged == null
        ? files.entries
        : {
            for (final MapEntry(:key, :value)
                in withoutManifest(merged.files).entries)
              key: FileEntry.bytes(value, type: m.files[key]?.type),
          };
    final title = merged?.title ?? m.title;
    final capabilities = merged?.capabilities ?? m.capabilities;
    final publishedSha256 = merged == null
        ? files.contentSha256
        : contentHash({
            for (final MapEntry(:key, :value) in merged.files.entries)
              key: (sha256: sha256Hex(value), size: value.length),
          });
    final hasMigration = entries.containsKey('migrate.js');
    final tests = testPaths(entries.keys);

    // 1–5: the check, on a draft with a copy of the documents.
    if (stop) return const UpdateCancelled();
    onStep?.call(UpdateStep.data);
    final DraftInfo draft;
    try {
      draft = await api.createDraft(
        of: aid,
        title: title,
        capabilities: capabilities,
        entries: entries,
        // The update's code is not published yet: it fetches nothing.
        inheritNet: false,
      );
    } on ClaxApiError catch (e) {
      return UpdateCheckFailed(
        UpdateStep.data,
        'its files could not be served',
        details: [e.message],
      );
    }
    final truncated = draft.docsTruncated;
    final (UpdateStep, String, List<String>)? failed;
    try {
      failed = await _check(
        draft.id,
        hasMigration: hasMigration,
        tests: tests,
        from: from,
        to: m.version,
        onStep: onStep,
        cancelled: cancelled,
      );
    } finally {
      try {
        await api.deleteDraft(draft.id);
      } on Object catch (e) {
        _log('deleting update draft ${draft.id} failed: $e');
      }
    }
    if (stop) return const UpdateCancelled();
    if (failed != null) {
      final (step, reason, details) = failed;
      return UpdateCheckFailed(
        step,
        reason,
        details: details,
        truncated: truncated,
      );
    }
    if (confirm != null && !await confirm()) return const UpdateCancelled();

    // 6–7: publish, then migrate the live documents under a snapshot.
    final meta = SnapshotMeta(
      base: base,
      contentSha256: publishedSha256,
      title: live.artifact.title,
      icon: live.artifact.icon,
      capabilities: live.artifact.capabilities,
      backOn: from,
    );
    if (hasMigration) {
      try {
        await snapshots.take(aid, meta);
      } on Object catch (e) {
        snapshots.discard(aid);
        return UpdateRefused(
          'Your data could not be saved before the update.',
          details: ['$e'],
        );
      }
    }
    _writePending(
      aid,
      bundle,
      publishedSha256,
      origin: (origin?.names(entry.link) ?? false) ? origin : null,
      keepsChanges: merged != null,
    );
    onStep?.call(UpdateStep.publish);
    PublishResult? published;
    Object? refused;
    try {
      // Under the store's lock: no other update of this lineage lands
      // between the re-check and the publish.
      await store.update(aid, (now) async {
        if (_mismatch(now, m, origin) case final why?) {
          refused = UpdateRefused(why);
          return null;
        }
        published = await api.publish(
          aid,
          PublishRequest(
            ifVersion: base,
            title: title,
            icon: m.icon.word,
            capabilities: capabilities,
            label: merged == null
                ? 'Update v${m.version}'
                : 'Your version with v${m.version}',
            note: merged == null
                ? files.noted(
                    'Updated to v${m.version} from '
                    '${m.author.name ?? keyFingerprint(m.author.key) ?? 'its author'}',
                  )
                : 'Brought v${m.version} into your version: ${merged.note}',
            entries: {
              ...entries,
              for (final p in live.current?.files.keys ?? const <String>[])
                if (!entries.containsKey(p)) p: null,
            },
          ),
        );
        return null;
      });
    } on ClaxApiError catch (e) {
      refused = UpdateRefused(
        e.isConflict
            ? '${live.artifact.title} changed while the update was checked. '
                  'Try again.'
            : 'The update could not be published.',
        details: [e.message],
      );
    }
    final landed = published;
    if (landed == null) {
      snapshots.discard(aid);
      _dropPending(aid);
      return refused as UpdateRefused? ??
          const UpdateRefused('The app was forgotten during the update.');
    }
    if (hasMigration) {
      snapshots.writeMeta(
        aid,
        (snapshots.meta(aid) ?? meta).copyWith(published: landed.version.n),
      );
      onStep?.call(UpdateStep.liveMigrate);
      final error = await _migrate(
        _page(aid),
        from,
        m.version,
        onWrite: (w) => snapshots.journal(aid, w),
      );
      if (error != null) {
        _dropPending(aid);
        final restored = await _putBack(aid);
        return UpdateRolledBack(from, details: [error], restored: restored);
      }
      // The snapshot goes first: a crash after it finishes from the
      // pending release, never rolls a passed update back.
      snapshots.discard(aid);
    }

    // The update stood: what the person accepted is approved. A rolled-back
    // update returned above, leaving no approval.
    if (confirm != null) {
      final shown = addedCapabilities(live.artifact.capabilities, capabilities);
      await approveCapabilities(
        api,
        aid,
        landed.pendingCapabilities,
        shownCapabilities: shown.capabilities,
        shownHosts: shown.hosts,
        log: _log,
      );
    }

    // 8: the lineage.
    final notLinked = await _applyPending(aid);
    return UpdateLanded(
      m.version,
      truncated: truncated,
      notLinked: notLinked,
    );
  }

  /// Keeps [bundle] and its release while it lands: the bundle's content
  /// hash and [published], the hash of the files published for it, and
  /// the public version of [origin], the linked copy it came from. With
  /// [keepsChanges] (the person's changes combined in) the published
  /// files are not recorded as a release.
  void _writePending(
    String aid,
    Bundle bundle,
    String published, {
    BundleOrigin? origin,
    bool keepsChanges = false,
  }) {
    final m = bundle.manifest;
    _pendingDir.createSync(recursive: true);
    _syncWrite(_pendingBundle(aid), bundle.bytes);
    _syncWrite(
      _pending(aid),
      utf8.encode(
        jsonEncode({
          'content_sha256': m.contentSha256,
          'published_sha256': published,
          'version': m.version,
          'title': m.title,
          'capabilities': m.capabilities,
          'name': m.author.name,
          'live_from': VersionSource(
            key: m.author.key,
            who:
                origin?.head.bundle?.publishedBy.handle ??
                m.author.name ??
                keyFingerprint(m.author.key) ??
                m.author.key,
            origin: origin?.remoteOrigin,
            account: origin?.head.bundle?.publishedBy,
          ).toJson(),
          if (keepsChanges) 'keeps_changes': true,
          if (origin != null) ...{
            'remote_artifact_id': origin.remoteArtifactId,
            'remote_version': origin.remoteVersion,
          },
        }),
      ),
    );
  }

  void _dropPending(String aid) {
    for (final f in [_pending(aid), _pendingBundle(aid)]) {
      if (f.existsSync()) f.deleteSync();
    }
  }

  /// Writes the pending release of [aid] into its lineage entry and keeps
  /// its bundle, then drops it. A release taken from the linked public
  /// copy moves the link to it: its base to the public version taken (never
  /// back), its taken bundle version to the release's, and its merged hash
  /// to the files published when they keep the person's changes (Bring vN
  /// into mine), else none.
  ///
  /// The link's base moves here only once the core's link moved to the
  /// same public version ([relink]); a refusal leaves both where they
  /// were and is answered, for the person.
  Future<String?> _applyPending(String aid) async {
    final p = jsonDecode(_pending(aid).readAsStringSync()) as Map;
    final caps = p['capabilities'];
    final version = p['version'] as int;
    final raid = p['remote_artifact_id'];
    final remoteVersion = p['remote_version'];
    String? notLinked;
    final was = (await store.entry(aid))?.link;
    if (was != null &&
        raid is String &&
        remoteVersion is int &&
        was.remoteArtifactId == raid &&
        remoteVersion > was.baseRemoteVersion) {
      if ((was.remoteId, relink) case (final rid?, final link?)) {
        try {
          await link(
            rid,
            aid,
            RemoteApp(
              artifactId: raid,
              title: p['title'] as String,
              url: was.url,
              version: remoteVersion,
              bundleVersion: version,
              access: was.isAuthor ? 'editor' : 'viewer',
              general: 'restricted',
            ),
            remoteVersion,
          );
        } on ClaxApiError catch (e) {
          _log('the core could not move $aid to public v$remoteVersion: '
              '${e.message}');
          notLinked = e.message;
        }
      }
    }
    final base = notLinked == null;
    AppLink? moved(AppLink? l) =>
        l == null || raid is! String || remoteVersion is! int ||
            l.remoteArtifactId != raid
        ? l
        : l.copyWith(
            baseRemoteVersion: base
                ? max(l.baseRemoteVersion, remoteVersion)
                : l.baseRemoteVersion,
            takenBundleVersion: () => version,
            mergedSha256: () => p['keeps_changes'] == true
                ? p['published_sha256'] as String?
                : null,
          );
    await store.update(
      aid,
      (now) async => now
          ?.withRelease(
            ReleaseState(
              contentSha256: p['content_sha256'] as String,
              title: p['title'] as String,
              capabilities: caps is Map<String, Object?> ? caps : const {},
            ),
            version: version,
            published: p['keeps_changes'] == true
                ? null
                : p['published_sha256'] as String?,
          )
          .copyWith(
            name: () => p['name'] as String?,
            updatesStopped: false,
            stoppedAt: () => null,
            link: () => moved(now.link),
            liveFrom: () =>
                VersionSource.fromJson(p['live_from']) ?? now.liveFrom,
          ),
    );
    await store.keepBundle(aid, _pendingBundle(aid).readAsBytesSync());
    _dropPending(aid);
    return notLinked;
  }

  static void _syncWrite(File target, List<int> bytes) {
    final tmp = File('${target.path}.tmp');
    final out = tmp.openSync(mode: FileMode.write);
    try {
      out.writeFromSync(bytes);
      out.flushSync();
    } finally {
      out.closeSync();
    }
    tmp.renameSync(target.path);
  }

  /// Puts back each interrupted update (§8.4), once the daemon is up.
  ///
  /// A pending release whose version is live is written into the lineage
  /// (the crash came after the publish); one that is not live is dropped.
  /// A kept snapshot is restored, documents and `base`'s version, title,
  /// icon and capabilities, only when its sidecar proves the live version
  /// is still the one the update published and no restore of it failed
  /// before; otherwise it is kept and reported not [RecoveredUpdate.restored]
  /// for the person to choose. A snapshot whose app is gone is deleted.
  Future<List<RecoveredUpdate>> recoverInterrupted() async {
    final out = <RecoveredUpdate>[];
    for (final aid in snapshots.pending()) {
      try {
        final r = await _recoverSnapshot(aid);
        if (r != null) out.add(r);
      } on Object catch (e) {
        _log('restoring the interrupted update of $aid failed: $e');
      }
    }
    if (_pendingDir.existsSync()) {
      for (final f in _pendingDir.listSync()) {
        if (f is! File || !f.path.endsWith('.json')) continue;
        final aid = f.uri.pathSegments.last.replaceFirst('.json', '');
        try {
          await _recoverPending(aid);
        } on Object catch (e) {
          _log('finishing the update of $aid failed: $e');
        }
      }
    }
    return out;
  }

  Future<RecoveredUpdate?> _recoverSnapshot(String aid) async {
    final ArtifactDetail d;
    try {
      d = await api.getArtifact(aid);
    } on ClaxApiError catch (e) {
      if (e.isNotFound) snapshots.discard(aid);
      return null;
    }
    final meta = snapshots.meta(aid);
    final now = d.artifact.currentVersion;
    if (meta == null || (meta.published == null && now == meta.base)) {
      // Nothing was published, so migrate.js never ran on the live app.
      snapshots.discard(aid);
      _dropPending(aid);
      return null;
    }
    final proven =
        !meta.failed &&
        meta.published == now &&
        d.current?.contentSha256 == meta.contentSha256;
    if (proven && await _putBack(aid)) {
      _dropPending(aid);
      return RecoveredUpdate(aid: aid, title: meta.title, backOn: meta.backOn);
    }
    return RecoveredUpdate(
      aid: aid,
      title: meta.title,
      backOn: meta.backOn,
      restored: false,
    );
  }

  Future<void> _recoverPending(String aid) async {
    if (snapshots.has(aid)) return; // The person decides first.
    final ArtifactDetail d;
    try {
      d = await api.getArtifact(aid);
    } on ClaxApiError catch (e) {
      if (e.isNotFound) _dropPending(aid);
      return;
    }
    final p = jsonDecode(_pending(aid).readAsStringSync()) as Map;
    if (d.current?.contentSha256 ==
        (p['published_sha256'] ?? p['content_sha256'])) {
      await _applyPending(aid);
    } else {
      _dropPending(aid);
    }
  }

  /// The person's "Restore" for a kept snapshot: the documents back as
  /// they were before the update, and `base`'s version and declaration
  /// when the live version is still the update's. False when it failed
  /// (the snapshot stays).
  Future<bool> restoreKept(String aid) async {
    try {
      return await _hold(aid, () => _putBack(aid, force: true));
    } on Object catch (e) {
      _log('restoring $aid failed: $e');
      return false;
    }
  }

  /// The person's "Discard" for a kept snapshot: keeps the app and its
  /// documents as they are.
  Future<void> discardKept(String aid) async {
    snapshots.discard(aid);
    _dropPending(aid);
  }

  /// Restores [aid]'s documents from its snapshot and its version to the
  /// snapshot's `base` (files, title, icon and capabilities, restored
  /// pinned to the update's version), then deletes the snapshot.
  /// The version is put back only while the live version is the update's
  /// (always when not [force]d, which callers check first). False when it
  /// failed: the snapshot stays, marked failed, for the person.
  Future<bool> _putBack(String aid, {bool force = false}) async {
    final meta = snapshots.meta(aid);
    if (meta == null) return false;
    try {
      await snapshots.restore(aid);
      await _replayPrivate(aid);
      final d = await api.getArtifact(aid);
      final now = d.artifact.currentVersion;
      if (!force ||
          (meta.published == now &&
              d.current?.contentSha256 == meta.contentSha256)) {
        await _rollBackVersion(aid, d, meta);
      }
      snapshots.discard(aid);
      return true;
    } on Object catch (e) {
      _log('rolling back the update of $aid failed: $e');
      try {
        snapshots.writeMeta(aid, meta.copyWith(failed: true));
      } on Object catch (e) {
        _log('marking the snapshot of $aid failed: $e');
      }
      return false;
    }
  }

  /// Puts back the person's private `data/users/` documents the live
  /// migration wrote, from its journal, through the migration view (the
  /// token cannot reach them). The owner's documents come back from the
  /// snapshot; their journal entries are only checked against it.
  Future<void> _replayPrivate(String aid) async {
    final writes = snapshots.journalEntries(aid);
    final kept = {for (final d in await snapshots.read(aid)) d.path: d.data};
    for (final w in writes) {
      if (w.path.startsWith('data/users/')) continue;
      if (!jsonEquals(kept[w.path], w.before)) {
        _log('the journal of $aid differs from its snapshot at ${w.path}');
      }
    }
    final private = [
      for (final w in writes)
        if (w.path.startsWith('data/users/')) w,
    ];
    if (private.isEmpty) return;
    await inspector.restoreWrites(_page(aid), private);
  }

  /// Restores `base` over [d]'s current version with its title, icon and
  /// capabilities, in one version (`versions:restore` with the
  /// declaration): the base's files come back as history, unchecked, so
  /// an icon an older build took and a publish now refuses still rolls
  /// back.
  Future<void> _rollBackVersion(
    String aid,
    ArtifactDetail d,
    SnapshotMeta meta,
  ) async {
    if (!d.versions.any((v) => v.n == meta.base)) {
      throw StateError('version ${meta.base} is gone');
    }
    await _ensureSession?.call();
    await api.restoreVersion(
      aid,
      meta.base,
      ifVersion: d.artifact.currentVersion,
      title: meta.title,
      icon: meta.icon,
      capabilities: meta.capabilities,
      label: 'Back to v${meta.backOn}',
      note: 'The update did not finish; back on v${meta.backOn}',
    );
  }

  /// Takes [entries] (a path's null removes it), [title] and
  /// [capabilities] as app [aid]'s next version, with no release behind
  /// them (spec 2026-10-21-app-push-pull §5.10: a public version made on
  /// the web, or two versions combined): checked first on a draft with a
  /// copy of the documents ([checkDraft]: the page and its tests; no
  /// `migrate.js` runs, as no release changes), then, once [confirm] (when
  /// given) agrees, published pinned to the version checked, as [label]
  /// with [note]. The lineage is left as it is. Never throws.
  Future<UpdateOutcome> takeFiles(
    String aid, {
    required Map<String, FileEntry?> entries,
    required String title,
    required Map<String, Object?> capabilities,
    required String label,
    required String note,
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
    Future<bool> Function()? confirm,
  }) async {
    try {
      if (_busy(aid)) {
        return const UpdateRefused('Wait for the change in progress to land.');
      }
      return await _hold(aid, () async {
        updating.value = {...updating.value, aid};
        try {
          return await _takeFiles(
            aid,
            entries,
            title,
            capabilities,
            label,
            note,
            onStep,
            cancelled,
            confirm,
          );
        } finally {
          updating.value = {...updating.value}..remove(aid);
        }
      });
    } on Object catch (e) {
      _log('taking files into $aid failed: $e');
      return UpdateRefused('The version could not be taken.', details: ['$e']);
    }
  }

  Future<UpdateOutcome> _takeFiles(
    String aid,
    Map<String, FileEntry?> entries,
    String title,
    Map<String, Object?> capabilities,
    String label,
    String note,
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
    Future<bool> Function()? confirm,
  ) async {
    var stop = false;
    cancelled?.then((_) => stop = true, onError: (Object _) => stop = true);
    final ArtifactDetail live;
    try {
      live = await api.getArtifact(aid);
    } on ClaxApiError catch (e) {
      return UpdateRefused('The app could not be read.', details: [e.message]);
    }
    final base = live.artifact.currentVersion;
    final version = (await store.entry(aid))?.version ?? 0;
    // Never clax.json: `capabilities` declares, and the daemon writes it.
    final sent = withoutManifest(entries);
    final kept = {
      for (final MapEntry(:key, :value) in sent.entries) key: ?value,
    };
    if (stop) return const UpdateCancelled();
    onStep?.call(UpdateStep.data);
    final DraftInfo draft;
    try {
      draft = await api.createDraft(
        of: aid,
        title: title,
        capabilities: capabilities,
        entries: kept,
        // Its code is not published yet: it fetches nothing.
        inheritNet: false,
      );
    } on ClaxApiError catch (e) {
      return UpdateCheckFailed(
        UpdateStep.data,
        'its files could not be served',
        details: [e.message],
      );
    }
    final DraftCheckFailure? failed;
    try {
      failed = await checkDraft(
        draft.id,
        // No release changes, so no data step runs.
        paths: kept.keys.where((p) => p != 'migrate.js'),
        from: version,
        to: version,
        onStep: onStep,
        cancelled: cancelled,
      );
    } finally {
      try {
        await api.deleteDraft(draft.id);
      } on Object catch (e) {
        _log('deleting draft ${draft.id} failed: $e');
      }
    }
    if (stop) return const UpdateCancelled();
    if (failed != null) {
      return UpdateCheckFailed(
        failed.step,
        failed.reason,
        details: failed.details,
        truncated: draft.docsTruncated,
      );
    }
    if (confirm != null && !await confirm()) return const UpdateCancelled();
    onStep?.call(UpdateStep.publish);
    try {
      final r = await api.publish(
        aid,
        PublishRequest(
          ifVersion: base,
          title: title,
          icon: live.artifact.icon,
          capabilities: capabilities,
          label: label,
          note: note,
          entries: {
            ...sent,
            for (final p in live.current?.files.keys ?? const <String>[])
              if (!sent.containsKey(p)) p: null,
          },
        ),
      );
      if (confirm != null) {
        final shown = addedCapabilities(live.artifact.capabilities, capabilities);
        await approveCapabilities(
          api,
          aid,
          r.pendingCapabilities,
          shownCapabilities: shown.capabilities,
          shownHosts: shown.hosts,
          log: _log,
        );
      }
      return FilesTaken(
        r.version.n,
        r.version.contentSha256 ??
            contentHash({
              for (final MapEntry(:key, :value) in kept.entries)
                key: (sha256: sha256Hex(value.bytes), size: value.bytes.length),
            }),
      );
    } on ClaxApiError catch (e) {
      return UpdateRefused(
        e.isConflict
            ? '${live.artifact.title} changed while it was checked. Try again.'
            : 'The version could not be published.',
        details: [e.message],
      );
    }
  }

  /// Checks draft [draftId] as an update's draft is checked (§8.2 steps
  /// 2–5), the draft holding a copy of the app's documents: `migrate.js`
  /// from version [from] to [to] when [paths] has one, the page, then its
  /// `tests/*.test.js`, telling [onStep] each step. The failure, or null on
  /// a pass. The draft is left as it is; the caller deletes it.
  Future<DraftCheckFailure?> checkDraft(
    String draftId, {
    required Iterable<String> paths,
    required int from,
    required int to,
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
  }) async {
    final failed = await _check(
      draftId,
      hasMigration: paths.contains('migrate.js'),
      tests: testPaths(paths),
      from: from,
      to: to,
      onStep: onStep,
      cancelled: cancelled,
    );
    if (failed == null) return null;
    final (step, reason, details) = failed;
    return DraftCheckFailure(step, reason, details);
  }

  /// The test files among [paths] (`tests/*.test.js`), sorted.
  static List<String> testPaths(Iterable<String> paths) => [
    for (final p in paths)
      if (RegExp(r'^tests/[^/].*\.test\.js$').hasMatch(p)) p,
  ]..sort();

  /// The check's failing step, reason and details, or null on a pass.
  Future<(UpdateStep, String, List<String>)?> _check(
    String draftId, {
    required bool hasMigration,
    required List<String> tests,
    required int from,
    required int to,
    void Function(UpdateStep)? onStep,
    Future<void>? cancelled,
  }) async {
    final url = _page(draftId);
    if (hasMigration) {
      onStep?.call(UpdateStep.migrate);
      final error = await _migrate(url, from, to, cancelled: cancelled);
      if (error != null) {
        return (UpdateStep.migrate, 'its data step failed', [error]);
      }
    }
    onStep?.call(UpdateStep.page);
    try {
      final seen = await inspector.inspect(url, cancelled: cancelled);
      final uncaught = seen.uncaught.toList();
      if (uncaught.isNotEmpty) {
        return (
          UpdateStep.page,
          'its page shows errors',
          [for (final e in uncaught) e.text],
        );
      }
    } on InspectorError catch (e) {
      return (UpdateStep.page, 'its page did not load', [e.message]);
    }
    if (tests.isNotEmpty) {
      onStep?.call(UpdateStep.tests);
      try {
        final run = await inspector.runTests(
          url,
          files: tests,
          perTest: perTestLimit,
          total: testLimit,
          cancelled: cancelled,
        );
        if (run.failed.isNotEmpty) {
          return (
            UpdateStep.tests,
            run.failed.length == 1
                ? 'a check failed'
                : '${run.failed.length} checks failed',
            [for (final f in run.failed) '${f.name}: ${f.error}'],
          );
        }
        if (run.skipped > 0) {
          return (
            UpdateStep.tests,
            'its checks did not all run',
            ['${run.skipped} checks did not run in time'],
          );
        }
      } on InspectorError catch (e) {
        return (UpdateStep.tests, 'its checks did not run', [e.message]);
      }
    }
    return null;
  }

  /// Runs `migrate.js` at [url], journaled through [onWrite] when given;
  /// the error, or null when it finished.
  Future<String?> _migrate(
    Uri url,
    int from,
    int to, {
    Future<void> Function(MigrationWrite w)? onWrite,
    Future<void>? cancelled,
  }) async {
    try {
      final run = await inspector.runMigration(
        url,
        from: from,
        to: to,
        total: migrationLimit,
        onWrite: onWrite,
        cancelled: cancelled,
      );
      return run.ok ? null : (run.error ?? 'the migration failed');
    } on InspectorError catch (e) {
      return e.message;
    }
  }

  /// `<base>/a/<aid>?app=1&inspect=1`, as the render tool serves a draft.
  Uri _page(String aid) {
    final b = baseUrl();
    return b.replace(
      pathSegments: [...b.pathSegments.where((p) => p.isNotEmpty), 'a', aid],
      query: 'app=1&inspect=1',
    );
  }
}
