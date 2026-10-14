/// Updating an installed app from a later bundle of its lineage: checked
/// on a draft with a copy of the person's data, published only on a pass
/// (bundles spec §3.3, §3.4, §8.2–8.4).
library;

import '../agent/inspector.dart';
import '../agent/tools/render_tool.dart' show perTestLimit, testLimit;
import '../clax/api.dart';
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

/// An update interrupted during its live migration, put back at start
/// on lineage version [backOn] (§8.4), for the one-time notice.
class RecoveredUpdate {
  const RecoveredUpdate({
    required this.aid,
    required this.title,
    required this.backOn,
  });

  final String aid;
  final String title;
  final int backOn;
}

/// Updates installed apps from bundles.
class Updater {
  Updater({
    required this.api,
    required this.store,
    required this.inspector,
    required this.baseUrl,
    required this.snapshots,
    bool Function(String aid)? busy,
    this.migrationLimit = const Duration(seconds: 30),
    void Function(String line)? log,
  }) : _busy = busy ?? ((_) => false),
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
  final void Function(String) _log;

  /// Marks [aid]'s lineage `updates: stopped` ("Keep mine").
  Future<void> keepMine(String aid) => store.stopUpdates(aid);

  /// Updates app [aid] to [bundle] (§8.2), telling [onStep] each step.
  /// Never throws for a failed check or refusal: the outcome says.
  Future<UpdateOutcome> update(
    String aid,
    Bundle bundle, {
    void Function(UpdateStep step)? onStep,
  }) async {
    if (_busy(aid)) {
      return const UpdateRefused('Wait for the change in progress to land.');
    }
    final m = bundle.manifest;
    final entry = await store.entry(aid);
    if (entry == null) {
      return const UpdateRefused('This app was not installed from a bundle.');
    }
    final ArtifactDetail live;
    try {
      live = await api.getArtifact(aid);
    } on ClaxApiError catch (e) {
      return UpdateRefused('The app could not be read.', details: [e.message]);
    }
    final base = live.artifact.currentVersion;
    final from = entry.version;
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
    final failed = await _check(
      draft.id,
      hasMigration: hasMigration,
      tests: tests,
      from: from,
      to: m.version,
      onStep: onStep,
    );
    try {
      await api.deleteDraft(draft.id);
    } on ClaxApiError catch (e) {
      _log('deleting update draft ${draft.id} failed: $e');
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
    if (hasMigration) {
      try {
        await snapshots.take(
          aid,
          SnapshotMeta(
            base: base,
            contentSha256: m.contentSha256,
            title: live.artifact.title,
            backOn: from,
          ),
        );
      } on Object catch (e) {
        snapshots.discard(aid);
        return UpdateRefused(
          'Your data could not be saved before the update.',
          details: ['$e'],
        );
      }
    }
    onStep?.call(UpdateStep.publish);
    final PublishResult published;
    try {
      published = await api.publish(
        aid,
        PublishRequest(
          ifVersion: base,
          title: m.title,
          icon: m.icon.word,
          capabilities: m.capabilities,
          label: 'Update v${m.version}',
          note:
              'Updated to v${m.version} from ${m.author.name ?? 'its author'}',
          entries: {
            ...entries,
            for (final p in _current(live)?.files.keys ?? const <String>[])
              if (!entries.containsKey(p)) p: null,
          },
        ),
      );
    } on ClaxApiError catch (e) {
      snapshots.discard(aid);
      return UpdateRefused(
        e.isConflict
            ? '${live.artifact.title} changed while the update was checked. '
                  'Try again.'
            : 'The update could not be published.',
        details: [e.message],
      );
    }
    if (hasMigration) {
      onStep?.call(UpdateStep.liveMigrate);
      final error = await _migrate(_page(aid), from, m.version);
      if (error != null) {
        final restored = await _putBack(aid, base, published.version.n);
        return UpdateRolledBack(from, details: [error], restored: restored);
      }
      snapshots.discard(aid);
    }

    // 8: the lineage.
    await store.put(
      aid,
      entry
          .withRelease(
            ReleaseState(
              contentSha256: m.contentSha256,
              title: m.title,
              capabilities: m.capabilities,
            ),
            version: m.version,
          )
          .copyWith(
            name: () => m.author.name,
            updatesStopped: false,
            stoppedAt: () => null,
          ),
    );
    await store.keepBundle(aid, bundle.bytes);
    return UpdateLanded(m.version, truncated: truncated);
  }

  /// Puts back each update interrupted during its live migration (§8.4):
  /// the documents from the snapshot, and the version before the update
  /// when the live version is the update's. Run once at the app's start,
  /// after the daemon is up. A snapshot whose app is gone is deleted; one
  /// that cannot be restored now is kept for the next start.
  Future<List<RecoveredUpdate>> recoverInterrupted() async {
    final out = <RecoveredUpdate>[];
    for (final aid in snapshots.pending()) {
      final meta = snapshots.meta(aid);
      try {
        final ArtifactDetail d;
        try {
          d = await api.getArtifact(aid);
        } on ClaxApiError catch (e) {
          if (e.isNotFound) snapshots.discard(aid);
          continue;
        }
        if (meta == null) {
          snapshots.discard(aid);
          continue;
        }
        final now = d.artifact.currentVersion;
        if (now != meta.base &&
            _current(d)?.contentSha256 == meta.contentSha256) {
          await api.restoreVersion(aid, meta.base, ifVersion: now);
        }
        await snapshots.restore(aid);
        snapshots.discard(aid);
        out.add(
          RecoveredUpdate(aid: aid, title: meta.title, backOn: meta.backOn),
        );
      } on Object catch (e) {
        _log('restoring the interrupted update of $aid failed: $e');
      }
    }
    return out;
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

  /// Runs `migrate.js` at [url]; the error, or null when it finished.
  Future<String?> _migrate(Uri url, int from, int to) async {
    try {
      final run = await inspector.runMigration(
        url,
        from: from,
        to: to,
        total: migrationLimit,
      );
      return run.ok ? null : (run.error ?? 'the migration failed');
    } on InspectorError catch (e) {
      return e.message;
    }
  }

  /// Restores [aid]'s documents from the snapshot and its version to
  /// [base] (published as [published]); false when that failed, leaving
  /// the snapshot for [recoverInterrupted].
  Future<bool> _putBack(String aid, int base, int published) async {
    try {
      await snapshots.restore(aid);
      await api.restoreVersion(aid, base, ifVersion: published);
      snapshots.discard(aid);
      return true;
    } on Object catch (e) {
      _log('rolling back the update of $aid failed: $e');
      return false;
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

  static VersionInfo? _current(ArtifactDetail d) => d.current;
}
