/// Checking linked apps against their public copies (spec
/// 2026-10-21-app-push-pull §5.4, §5.7): the head each copy is at, the sync
/// state it makes, and the automatic updates it starts.
library;

import 'dart:async';

import 'package:flutter/foundation.dart' show ValueListenable, ValueNotifier;

import '../bundle/installer.dart' show OpenedBundle, shareableCapabilities;
import '../bundle/lineage_store.dart';
import '../bundle/updater.dart';
import '../clax/api.dart' show ClaxApi, ClaxApiError;
import '../platform/clock.dart';
import 'link_remote.dart';
import 'models.dart';
import 'puller.dart';
import 'remote_api.dart';
import 'sync_state.dart';

/// Where a linked app stands, as the checker last worked it out.
class LinkedStatus {
  const LinkedStatus({
    required this.state,
    this.head,
    this.localVersion,
    this.publicVersion,
  });

  final SyncState state;

  /// The public copy's head: the newest read so far, kept while later
  /// reads fail; null when none has been read.
  final RemoteHead? head;

  /// The live local version, when it could be read.
  final int? localVersion;

  /// The local version this device last published to the copy (an
  /// author's), or null.
  final int? publicVersion;
}

/// An app's live local version, as the checker weighs it.
class LiveApp {
  const LiveApp({
    required this.version,
    required this.contentSha256,
    required this.capabilities,
  });

  final int version;

  /// `sha256:<hex>` of the version's files.
  final String contentSha256;

  /// The capabilities as a bundle carries them ([shareableCapabilities]).
  final Map<String, Object?> capabilities;
}

/// Reads app `aid`'s live version, or null when it cannot be read.
typedef ReadLive = Future<LiveApp?> Function(String aid);

/// Fetches the newest bundle of [link]'s copy as an update of app `aid`
/// ([Puller.pull]: checked against the head, opened as from the copy);
/// it must never install a new app.
typedef FetchUpdate = Future<PullOutcome> Function(AppLink link);

/// Runs [opened] as an update of app `aid` ([Updater.update] with the
/// bundle's origin).
typedef RunUpdate =
    Future<UpdateOutcome> Function(String aid, OpenedBundle opened);

/// Reads [api]'s live version of an app, for [LinkedChecker.live].
ReadLive liveFrom(ClaxApi api) => (aid) async {
  final d = await api.getArtifact(aid);
  final v = d.current;
  if (v == null) return null;
  return LiveApp(
    version: v.n,
    contentSha256: v.contentSha256 ?? '',
    capabilities: shareableCapabilities(d.artifact.capabilities),
  );
};

/// Checks linked apps against their public copies and keeps each one's
/// [LinkedStatus] in [status].
///
/// - **When.** [onForeground] (the app back in front) reads the heads of
///   the apps linked at a remote at most once per [every] (15 minutes) per
///   remote, measured on [clock] from the last round that remote had;
///   [onOpen] (a linked app opening) and [checkNow] (after a publish, a
///   change landing or an update) read that app's head at once, whatever
///   the limit, and do not move it.
/// - **How.** A remote is matched by its origin ([LinkRemotes.at]): with
///   this device signed in there the head is read with its token
///   ([RemoteApi.head]), else without ([RemoteApi.anonymousHead]).
/// - **Failures.** A head that cannot be read (offline, signed out, the
///   copy gone) keeps the head read before, and the state is worked out
///   again from it; nothing records the failure, so nothing blocks launch
///   or the app. A head older than the one held (a stale answer) is
///   ignored, and [syncState] never offers a head at or below what this
///   device holds, so no ↓ comes from a cached head this device has
///   passed. Checks never throw.
/// - **Automatic updates.** An app in [SyncState.updateWaiting] whose link
///   says [AppLink.autoUpdate] and whose local copy is unmodified is
///   updated in place: [fetchUpdate], then [runUpdate], unless the update
///   adds a capability or a `net` host ([addedCapabilities]), which waits
///   for the person (the update offer). A head made on the web has no
///   bundle to update to: its Take theirs waits for the person too. One
///   app's update runs at a time.
///
/// Background refresh (WorkManager, BGAppRefresh) is not part of it: see
/// the record on this file.
class LinkedChecker {
  LinkedChecker({
    required this.api,
    required this.store,
    required this.clock,
    LinkRemotes? links,
    this.live,
    this.fetchUpdate,
    this.runUpdate,
    this.every = const Duration(minutes: 15),
    void Function(String line)? log,
  }) : _log = log ?? ((_) {}),
       links = links ?? LinkRemotes(api: api, store: store, log: log);

  final RemoteApi api;
  final LineageStore store;
  final Clock clock;

  /// Linked apps' remotes, by origin.
  final LinkRemotes links;

  /// Reads an app's live version; without it the live version is taken to
  /// be the head's and the app unmodified.
  final ReadLive? live;

  /// Fetches an update for an automatic update; without it, or without
  /// [runUpdate], none runs.
  final FetchUpdate? fetchUpdate;
  final RunUpdate? runUpdate;

  /// The least time between two [onForeground] rounds of one remote.
  final Duration every;
  final void Function(String) _log;

  final _status = ValueNotifier<Map<String, LinkedStatus>>(const {});

  /// Each linked app's status, by local artifact ID; an app not linked, or
  /// not checked yet, has none.
  ValueListenable<Map<String, LinkedStatus>> get status => _status;

  /// When each remote (by origin) last had a foreground round.
  final _rounds = <String, DateTime>{};

  /// The newest head read for each app, with the copy it is of.
  final _heads = <String, RemoteHead>{};

  /// Each app's last check, which the next one waits for.
  final _checks = <String, Future<void>>{};

  /// The apps an automatic update runs on.
  final _updating = <String>{};

  /// The app came to the foreground: reads the heads of the apps linked
  /// at each remote whose last round was [every] or longer ago.
  Future<void> onForeground() async {
    final Map<String, LineageEntry> all;
    try {
      all = await store.all();
    } on Object catch (e) {
      _log('the lineages could not be read: $e');
      return;
    }
    final now = clock.now();
    final due = <String>{};
    final skipped = <String>{};
    final aids = <String>[];
    for (final MapEntry(key: aid, value: e) in all.entries) {
      final l = e.link;
      if (l == null) continue;
      final o = originOf(l.remoteOrigin);
      if (skipped.contains(o)) continue;
      if (!due.contains(o)) {
        final last = _rounds[o];
        if (last != null && now.difference(last) < every) {
          skipped.add(o);
          continue;
        }
        due.add(o);
      }
      aids.add(aid);
    }
    for (final o in due) {
      _rounds[o] = now;
    }
    for (final aid in aids) {
      await _queue(aid);
    }
  }

  /// Linked app [aid] opened: reads its head now.
  Future<void> onOpen(String aid) => _queue(aid);

  /// Reads [aid]'s head now and works its state out again.
  Future<void> checkNow(String aid) => _queue(aid);

  Future<void> _queue(String aid) {
    final before = _checks[aid] ?? Future<void>.value();
    final run = before.then((_) => _check(aid));
    _checks[aid] = run;
    return run.whenComplete(() {
      if (identical(_checks[aid], run)) _checks.remove(aid);
    });
  }

  Future<void> _check(String aid) async {
    try {
      await _checkOnce(aid);
    } on Object catch (e) {
      _log('the check of $aid failed: $e');
    }
  }

  Future<void> _checkOnce(String aid) async {
    final entry = await store.entry(aid);
    final link = entry?.link;
    if (entry == null || link == null) {
      _heads.remove(aid);
      _set(aid, null);
      return;
    }
    var head = _heads[aid];
    if (head != null && head.artifactId != link.remoteArtifactId) head = null;
    try {
      final got = await _read(link);
      if (got.artifactId == link.remoteArtifactId &&
          (head == null || got.version >= head.version)) {
        head = got;
      }
    } on ClaxApiError catch (e) {
      _log('the head of $aid could not be read: ${e.message}');
    }
    if (head != null) _heads[aid] = head;
    LiveApp? now;
    try {
      now = await live?.call(aid);
    } on Object catch (e) {
      _log('$aid could not be read: $e');
    }
    final modified =
        now != null &&
        entry.isModified(
          contentSha256: now.contentSha256,
          capabilities: now.capabilities,
        );
    final state = syncState(
      link: link,
      liveContentSha256: now?.contentSha256 ?? head?.contentSha256 ?? '',
      entry: entry,
      head: head,
      modified: modified,
    );
    _set(
      aid,
      LinkedStatus(
        state: state,
        head: head,
        localVersion: now?.version,
        publicVersion: link.isAuthor ? link.published.lastOrNull : null,
      ),
    );
    if (state == SyncState.updateWaiting &&
        link.autoUpdate &&
        !modified &&
        !(head?.webMade ?? true) &&
        !_updating.contains(aid)) {
      await _autoUpdate(aid, entry, link);
    }
  }

  Future<RemoteHead> _read(AppLink link) async {
    final r = await links.at(link.remoteOrigin);
    return r != null
        ? api.head(r.id, link.remoteArtifactId)
        : api.anonymousHead(Uri.parse(link.url));
  }

  /// Updates [aid] to its copy's newest bundle, unless the update adds a
  /// capability or a `net` host, or the copy turns out modified.
  Future<void> _autoUpdate(String aid, LineageEntry entry, AppLink link) async {
    final fetch = fetchUpdate, run = runUpdate;
    if (fetch == null || run == null) return;
    _updating.add(aid);
    try {
      final got = await fetch(link);
      if (got is! PullUpdate) return;
      final o = got.opened;
      if (o.decision.aid != aid || o.decision.modified || o.decision.stopped) {
        return;
      }
      final added = addedCapabilities(
        o.live?.capabilities ?? entry.capabilities,
        o.manifest.capabilities,
      );
      if (!added.isEmpty) {
        _log(
          'the update of $aid asks for more '
          '(${[...added.capabilities, ...added.hosts].join(', ')}): '
          'it waits for the person',
        );
        return;
      }
      final out = await run(aid, o);
      _log('the automatic update of $aid: ${out.runtimeType}');
      if (out is UpdateLanded) await _checkOnce(aid);
    } on Object catch (e) {
      _log('the automatic update of $aid failed: $e');
    } finally {
      _updating.remove(aid);
    }
  }

  void _set(String aid, LinkedStatus? s) {
    final next = {..._status.value};
    if (s == null) {
      if (next.remove(aid) == null) return;
    } else {
      next[aid] = s;
    }
    _status.value = next;
  }
}
