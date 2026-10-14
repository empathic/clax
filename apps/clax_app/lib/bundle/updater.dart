/// Updating an installed app from a later bundle of its lineage: checked
/// on a draft with a copy of the person's data, published only on a pass
/// (bundles spec §3.3, §3.4, §8.2–8.4).
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart' show ValueNotifier;

import '../agent/inspector.dart';
import '../agent/tools/render_tool.dart' show perTestLimit, testLimit;
import '../clax/api.dart';
import '../platform/bundle_key.dart' show keyFingerprint;
import 'bundle_reader.dart';
import 'doc_snapshot.dart';
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
  const UpdateLanded(this.version, {this.truncated = false});

  final int version;
  final bool truncated;
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
    void Function(String line)? log,
  }) : _busy = busy ?? ((_) => false),
       _hold = hold ?? _noHold,
       _log = log ?? ((_) {});

  final ClaxApi api;
  final LineageStore store;
  final Inspector inspector;

  /// The daemon's base URL, as the render tool takes it.
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
  Future<UpdateOutcome> update(
    String aid,
    Bundle bundle, {
    void Function(UpdateStep step)? onStep,
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
          return await _update(aid, bundle, onStep);
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
  /// same lineage and key, installed, at a lower version.
  static String? _mismatch(LineageEntry? entry, Manifest m) {
    if (entry == null || entry.role != LineageRole.installed) {
      return 'This app was not installed from a bundle.';
    }
    if (entry.lineage != m.lineage || entry.key != m.author.key) {
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
  ) async {
    final m = bundle.manifest;
    final entry = await store.entry(aid);
    if (_mismatch(entry, m) case final why?) return UpdateRefused(why);
    final from = entry!.version;
    final ArtifactDetail live;
    try {
      live = await api.getArtifact(aid);
    } on ClaxApiError catch (e) {
      return UpdateRefused('The app could not be read.', details: [e.message]);
    }
    final base = live.artifact.currentVersion;
    final entries = {
      for (final MapEntry(:key, :value) in m.files.entries)
        key: FileEntry.bytes(bundle.files[key]!, type: value.type),
    };
    final hasMigration = m.files.containsKey('migrate.js');
    final tests = [
      for (final p in m.files.keys)
        if (RegExp(r'^tests/[^/].*\.test\.js$').hasMatch(p)) p,
    ]..sort();

    // 1–5: the check, on a draft with a copy of the documents.
    onStep?.call(UpdateStep.data);
    final DraftInfo draft;
    try {
      draft = await api.createDraft(
        of: aid,
        title: m.title,
        capabilities: m.capabilities,
        entries: entries,
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
      );
    } finally {
      try {
        await api.deleteDraft(draft.id);
      } on Object catch (e) {
        _log('deleting update draft ${draft.id} failed: $e');
      }
    }
    if (failed != null) {
      final (step, reason, details) = failed;
      return UpdateCheckFailed(
        step,
        reason,
        details: details,
        truncated: truncated,
      );
    }

    // 6–7: publish, then migrate the live documents under a snapshot.
    final meta = SnapshotMeta(
      base: base,
      contentSha256: m.contentSha256,
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
    _writePending(aid, bundle);
    onStep?.call(UpdateStep.publish);
    PublishResult? published;
    Object? refused;
    try {
      // Under the store's lock: no other update of this lineage lands
      // between the re-check and the publish.
      await store.update(aid, (now) async {
        if (_mismatch(now, m) case final why?) {
          refused = UpdateRefused(why);
          return null;
        }
        published = await api.publish(
          aid,
          PublishRequest(
            ifVersion: base,
            title: m.title,
            icon: m.icon.word,
            capabilities: m.capabilities,
            label: 'Update v${m.version}',
            note:
                'Updated to v${m.version} from '
                '${m.author.name ?? keyFingerprint(m.author.key) ?? 'its author'}',
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

    // 8: the lineage.
    await _applyPending(aid);
    return UpdateLanded(m.version, truncated: truncated);
  }

  void _writePending(String aid, Bundle bundle) {
    final m = bundle.manifest;
    _pendingDir.createSync(recursive: true);
    _syncWrite(_pendingBundle(aid), bundle.bytes);
    _syncWrite(
      _pending(aid),
      utf8.encode(
        jsonEncode({
          'content_sha256': m.contentSha256,
          'version': m.version,
          'title': m.title,
          'capabilities': m.capabilities,
          'name': m.author.name,
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
  /// its bundle, then drops it.
  Future<void> _applyPending(String aid) async {
    final p = jsonDecode(_pending(aid).readAsStringSync()) as Map;
    final caps = p['capabilities'];
    await store.update(
      aid,
      (now) async => now
          ?.withRelease(
            ReleaseState(
              contentSha256: p['content_sha256'] as String,
              title: p['title'] as String,
              capabilities: caps is Map<String, Object?> ? caps : const {},
            ),
            version: p['version'] as int,
          )
          .copyWith(
            name: () => p['name'] as String?,
            updatesStopped: false,
            stoppedAt: () => null,
          ),
    );
    await store.keepBundle(aid, _pendingBundle(aid).readAsBytesSync());
    _dropPending(aid);
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
    if (d.current?.contentSha256 == p['content_sha256']) {
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
  /// snapshot's `base` (files, title, icon and capabilities, published
  /// again pinned to the update's version), then deletes the snapshot.
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

  /// Publishes `base`'s files, title, icon and capabilities over [d]'s
  /// current version (a `versions:restore` would keep the update's
  /// declaration).
  Future<void> _rollBackVersion(
    String aid,
    ArtifactDetail d,
    SnapshotMeta meta,
  ) async {
    final base = d.versions.where((v) => v.n == meta.base).firstOrNull;
    if (base == null) throw StateError('version ${meta.base} is gone');
    final entries = <String, FileEntry?>{};
    for (final MapEntry(key: path, value: f) in base.files.entries) {
      final bytes = await api.readFileBytes(aid, base.n, path);
      if (bytes == null) throw StateError('$path of v${base.n} is gone');
      entries[path] = FileEntry.bytes(bytes, type: f.contentType);
    }
    for (final p in d.current?.files.keys ?? const <String>[]) {
      entries.putIfAbsent(p, () => null);
    }
    await _ensureSession?.call();
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: d.artifact.currentVersion,
        title: meta.title,
        icon: meta.icon,
        capabilities: meta.capabilities,
        label: 'Back to v${meta.backOn}',
        note: 'The update did not finish; back on v${meta.backOn}',
        entries: entries,
      ),
    );
  }

  /// The check's failing step, reason and details, or null on a pass.
  Future<(UpdateStep, String, List<String>)?> _check(
    String draftId, {
    required bool hasMigration,
    required List<String> tests,
    required int from,
    required int to,
    void Function(UpdateStep)? onStep,
  }) async {
    final url = _page(draftId);
    if (hasMigration) {
      onStep?.call(UpdateStep.migrate);
      final error = await _migrate(url, from, to);
      if (error != null) {
        return (UpdateStep.migrate, 'its data step failed', [error]);
      }
    }
    onStep?.call(UpdateStep.page);
    try {
      final seen = await inspector.inspect(url);
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
        );
        if (run.failed.isNotEmpty) {
          return (
            UpdateStep.tests,
            run.failed.length == 1
                ? 'a test failed'
                : '${run.failed.length} tests failed',
            [for (final f in run.failed) '${f.name}: ${f.error}'],
          );
        }
        if (run.skipped > 0) {
          return (
            UpdateStep.tests,
            'its tests did not all run',
            ['${run.skipped} tests did not run in time'],
          );
        }
      } on InspectorError catch (e) {
        return (UpdateStep.tests, 'its tests did not run', [e.message]);
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
  }) async {
    try {
      final run = await inspector.runMigration(
        url,
        from: from,
        to: to,
        total: migrationLimit,
        onWrite: onWrite,
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
