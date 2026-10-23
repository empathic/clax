/// Publishing a local version of an app to its public copy on a remote
/// (spec 2026-10-21-app-push-pull §5.3, §5.8, §5.13).
library;

import 'dart:async';
import 'dart:io';
import 'dart:math' show min;
import 'dart:typed_data';

import 'package:flutter/foundation.dart' show ValueNotifier;

import '../bundle/exporter.dart';
import '../bundle/installer.dart' show BundleFailure;
import '../bundle/lineage_store.dart';
import '../clax/api.dart' show ClaxApiError;
import '../platform/clock.dart';
import 'link_remote.dart';
import 'models.dart';
import 'remote_api.dart';
import 'remote_errors.dart';

/// What a publish came to.
sealed class PublishOutcome {
  const PublishOutcome();
}

/// The version is public version [remoteVersion] of the copy at [url], its
/// bundle beside it.
class Published extends PublishOutcome {
  const Published(this.url, this.remoteVersion);

  final String url;
  final int remoteVersion;
}

/// The public copy has versions this device did not publish: it is at
/// [remoteVersion]. Nothing was published.
class Moved extends PublishOutcome {
  const Moved(this.remoteVersion);

  final int remoteVersion;
}

/// The publish was refused, for the reason [message] gives in a sentence;
/// [details] holds the raw refusal, for the log and Developer tools.
class Refused extends PublishOutcome {
  const Refused(this.message, {this.details = const []});

  final String message;
  final List<String> details;
}

/// Local version [version] of app [aid] became public version
/// [remoteVersion] of its copy.
class PublishedVersion {
  const PublishedVersion({
    required this.aid,
    required this.version,
    required this.remoteVersion,
  });

  final String aid;
  final int version;
  final int remoteVersion;
}

/// A publish of local version [version] of app [aid] ended as [outcome].
class PublishEnded {
  const PublishEnded({
    required this.aid,
    required this.version,
    required this.outcome,
  });

  final String aid;
  final int version;
  final PublishOutcome outcome;
}

/// The sentence for an app whose files use assets (`UsesAssets`).
const usesAssetsMessage = 'This app uses files Clax can’t share yet.';

/// Publishes apps' versions to their public copies.
///
/// A publish (§5.3):
///
/// 1. builds the version's bundle as Export does ([Exporter.export],
///    [ExportMode.own]), with the documents only when `includeData` says
///    so for this publish, and for a linked app past both its lineage
///    version and the copy's latest bundle version (read from its head);
/// 2. pushes that local version as the copy's next public version
///    ([RemoteApi.publish]; the copy is made on the first publish);
/// 3. once the push is accepted, and only then, records the release in
///    the lineage and the link at the new public version, with the bundle
///    as its [AppLink.pendingBundle] and the bytes in
///    `<support>/publishing/<aid>.clax`;
/// 4. uploads the bundle beside that version ([RemoteApi.putBundle]) and
///    then drops the pending record.
///
/// A push or an upload that gets no answer (a `ClaxApiError` of status 0)
/// or a 5xx is tried again on [clock], after [firstRetry], doubling to
/// [maxRetry], until it lands, the app in [waiting] meanwhile; a lost
/// push answer is safe to send again (the core replays it). An upload cut
/// short by the app's end is finished by [resume] at the next launch.
///
/// A linked app is published through the remote signed in to at its
/// link's origin now ([LinkRemotes.resolve]), which a disconnect and a
/// later sign-in change; the core's link is made again on it first. While
/// no remote there is signed in to, a pending bundle is kept, and
/// [resume] lands it after the next sign-in.
///
/// Publishes of one app run one at a time, in the order asked: a second
/// waits for the first to end.
class Publisher {
  Publisher({
    required this.remote,
    required this.exporter,
    required this.store,
    required this.clock,
    this.authorName,
    this.firstRetry = const Duration(seconds: 2),
    this.maxRetry = const Duration(minutes: 5),
    void Function(String line)? log,
  }) : _log = log ?? ((_) {});

  final RemoteApi remote;
  final Exporter exporter;
  final LineageStore store;
  final Clock clock;

  /// The name a published bundle carries as its author's.
  final Future<String?> Function()? authorName;
  final Duration firstRetry;
  final Duration maxRetry;
  final void Function(String) _log;

  /// Linked apps' remotes, resolved by their origins.
  late final links = LinkRemotes(api: remote, store: store, log: _log);

  /// The apps whose publish waits for a connection ("Waiting for a
  /// connection").
  final waiting = ValueNotifier<Set<String>>(const {});

  final _published = StreamController<PublishedVersion>.broadcast();

  /// Each publish that landed, as it lands, whoever asked for it (the
  /// publish sheet, an automatic publish, Publish vN and reply). A
  /// publish does not wait for its listeners.
  Stream<PublishedVersion> get published => _published.stream;

  final _ended = StreamController<PublishEnded>.broadcast();

  /// Each publish's outcome as it ends, landed or not, whoever asked for
  /// it: how a publish [publishOrWait] left to go on is heard of. A
  /// publish does not wait for its listeners.
  Stream<PublishEnded> get ended => _ended.stream;

  /// Publishes local [version] of [aid], a change that just landed, when
  /// its link says to publish new versions without asking
  /// ([AppLink.autoPublish]) and this device may publish there; null,
  /// publishing nothing, otherwise.
  Future<PublishOutcome?> autoPublish(String aid, int version) async {
    final link = (await store.entry(aid))?.link;
    if (link == null || !link.autoPublish || !link.isAuthor) return null;
    if (link.published.isNotEmpty && version <= link.published.last) {
      return null;
    }
    return publish(aid, version: version);
  }

  /// Publishes local [version] of [aid] as [publish] does, but answers
  /// null as soon as the publish waits for a connection ([waiting] holds
  /// the app, now or before it starts): the publish goes on by itself and
  /// [published] tells when it lands. Like [publish], it joins a publish of
  /// that version already on its way. Never throws.
  Future<PublishOutcome?> publishOrWait(String aid, {required int version}) {
    final run = publish(aid, version: version);
    if (waiting.value.contains(aid)) return Future.value();
    final answer = Completer<PublishOutcome?>();
    void waits() {
      if (!answer.isCompleted && waiting.value.contains(aid)) {
        answer.complete(null);
      }
    }

    waiting.addListener(waits);
    unawaited(
      run.then((out) {
        if (!answer.isCompleted) answer.complete(out);
      }),
    );
    return answer.future.whenComplete(() => waiting.removeListener(waits));
  }

  /// The uploads running, by app, so a publish and [resume] share one.
  final _landing = <String, Future<String?>>{};

  /// The last publish asked for, by app, which the next one waits for.
  final _publishing = <String, Future<void>>{};
  var _writes = 0;

  File _pendingFile(String aid) =>
      File('${store.support.path}/publishing/$aid.clax');

  /// Publishes local [version] of [aid] as its public copy's next version
  /// on its link's remote, or on [rid] (the only connected remote when
  /// null) for an app not linked yet. [autoPublish], when given, becomes
  /// the link's "Publish new versions" choice. Never throws: a refusal is
  /// the outcome.
  ///
  /// A publish of [version] already on its way (asked for and not ended)
  /// is joined, by the sheet, an automatic publish or Publish vN and reply
  /// alike, rather than sending the version twice. What decides it is what
  /// is pushed: only [includeData] starts a publish of its own, when the
  /// one on its way sends no data (an ask without data joins one with).
  /// [rid] names the remote of an app not linked yet; for a linked app it
  /// is the link's, so it does not matter, and an ask naming another
  /// remote for an app not linked starts its own. An [autoPublish] choice
  /// that joins is applied to the link once the joined publish lands. The
  /// latest such publish on its way is joined. Once a publish has ended,
  /// publishing the version again makes a new public version.
  Future<PublishOutcome> publish(
    String aid, {
    required int version,
    bool includeData = false,
    String? rid,
    bool? autoPublish,
  }) {
    final ask = (version: version, includeData: includeData, rid: rid);
    _OnTheWay? same, otherRemote;
    for (final w in (_onTheWay[aid] ?? const <_OnTheWay>[]).reversed) {
      final (a, _) = w;
      if (a.version != version || (includeData && !a.includeData)) continue;
      if (rid == null || a.rid == null || a.rid == rid) {
        same = w;
        break;
      }
      otherRemote ??= w;
    }
    if (same != null) return _choose(aid, same.$2, autoPublish);
    if (otherRemote case (_, final f)) {
      // Another remote was named: the same publish when the app is linked
      // (its link names the remote), else a publish of its own.
      return store.entry(aid).then(
        (e) => e?.link != null
            ? _choose(aid, f, autoPublish)
            : _start(aid, ask, autoPublish),
      );
    }
    return _start(aid, ask, autoPublish);
  }

  Future<PublishOutcome> _start(
    String aid,
    _Ask ask,
    bool? autoPublish,
  ) {
    final run = _queue(
      aid,
      version: ask.version,
      includeData: ask.includeData,
      rid: ask.rid,
      autoPublish: autoPublish,
    );
    final mine = (ask, run);
    (_onTheWay[aid] ??= []).add(mine);
    unawaited(
      run.then((out) {
        _ended.add(
          PublishEnded(aid: aid, version: ask.version, outcome: out),
        );
      }).whenComplete(() {
        final all = _onTheWay[aid];
        all?.remove(mine);
        if (all != null && all.isEmpty) _onTheWay.remove(aid);
      }),
    );
    return run;
  }

  /// [run], a publish being joined, with [autoPublish] (when given)
  /// applied to the link once it lands.
  Future<PublishOutcome> _choose(
    String aid,
    Future<PublishOutcome> run,
    bool? autoPublish,
  ) {
    if (autoPublish == null) return run;
    return run.then((out) async {
      if (out is Published) {
        await store.update(aid, (e) async {
          final l = e?.link;
          if (e == null || l == null) return null;
          return e.copyWith(link: () => l.copyWith(autoPublish: autoPublish));
        });
      }
      return out;
    });
  }

  /// The publishes asked for and not ended, by app.
  final _onTheWay = <String, List<_OnTheWay>>{};

  Future<PublishOutcome> _queue(
    String aid, {
    required int version,
    required bool includeData,
    required String? rid,
    required bool? autoPublish,
    bool asNew = false,
    String? title,
    String? forkedFrom,
    Future<PublishOutcome?> Function()? first,
  }) {
    final before = _publishing[aid] ?? Future<void>.value();
    final epoch = _epochs[aid] ?? 0;
    final run = before.then((_) async {
      if ((_epochs[aid] ?? 0) != epoch) return _stopped;
      if (await first?.call() case final refused?) return refused;
      return _publish(
        aid,
        version: version,
        includeData: includeData,
        rid: rid,
        autoPublish: autoPublish,
        asNew: asNew,
        title: title,
        forkedFrom: forkedFrom,
        epoch: epoch,
      );
    });
    final done = run.then<void>((_) {});
    _publishing[aid] = done;
    return run.whenComplete(() {
      if (identical(_publishing[aid], done)) _publishing.remove(aid);
    });
  }

  Future<PublishOutcome> _publish(
    String aid, {
    required int version,
    required bool includeData,
    required String? rid,
    required bool? autoPublish,
    bool asNew = false,
    String? title,
    String? forkedFrom,
    required int epoch,
  }) async {
    try {
      final AppLink? link;
      try {
        // A new app's publish goes as an app not linked yet.
        link = asNew
            ? null
            : await _retrying(aid, () => links.resolve(aid), epoch: epoch);
      } on ClaxApiError catch (e) {
        return _refused(e, await _hostOf(null, rid));
      }
      if (link != null && link.remoteId == null) {
        return Refused(
          'This device is not signed in to '
          '${Uri.parse(link.remoteOrigin).host}.',
        );
      }
      if (link != null && !link.isAuthor) {
        return const Refused(
          'Only its owner and the people who can edit it can publish this '
          'app.',
        );
      }
      // An earlier bundle still on its way goes first, so the copy's bundle
      // versions keep going up.
      if (link?.pendingBundle != null) {
        if (await _land(aid) case final why?) return Refused(why);
      }
      final ExportedBundle built;
      try {
        final installed =
            (await store.entry(aid))?.role == LineageRole.installed;
        built = await exporter.export(
          aid,
          // A new app is a fork of what it was: an installed app's own
          // fork, or a new lineage of an app made here.
          mode: asNew && installed ? ExportMode.fork : ExportMode.own,
          newLineage: asNew,
          version: version,
          includeData: includeData,
          authorName: await authorName?.call(),
          atLeast: link == null ? null : await _latestBundle(aid, link),
          record: false,
          title: title,
        );
      } on BundleFailure catch (e) {
        return Refused(e.message, details: e.details);
      }
      final PushResult pushed;
      try {
        pushed = await _retrying(
          aid,
          () => remote.publish(
            aid,
            version: version,
            rid: link?.remoteId ?? rid,
            title: title,
            forkedFrom: forkedFrom,
          ),
          epoch: epoch,
        );
      } on RemoteMoved catch (e) {
        return Moved(e.remoteVersion);
      } on UsesAssets {
        return const Refused(usesAssetsMessage);
      } on ClaxApiError catch (e) {
        return _refused(e, await _hostOf(link, rid));
      }
      if ((_epochs[aid] ?? 0) != epoch) {
        // Stopped while the push was out (Unpublish): its copy is not
        // linked here.
        _log('the publish of $aid was stopped after its push landed as '
            '${pushed.url}');
        return _stopped;
      }
      final m = built.manifest;
      final pending = PendingBundle(
        remoteVersion: pushed.remoteVersion,
        lineage: m.lineage,
        bundleVersion: m.version,
        contentSha256: m.contentSha256,
      );
      final file = _pendingFile(aid);
      await file.parent.create(recursive: true);
      final tmp = File('${file.path}.${pid}_${_writes++}.tmp');
      await tmp.writeAsBytes(built.bytes, flush: true);
      await tmp.rename(file.path);
      final made = built.entry!;
      // The export's entry is written now, the push accepted: an app
      // published for the first time has none before.
      await store.update(aid, (e) async {
        // A link to another copy than the one pushed to (the core's link
        // was ended meanwhile, so the push made a new copy) is not kept.
        final had = asNew ? null : e?.link;
        final was = had?.remoteArtifactId == pushed.remoteArtifactId
            ? had
            : null;
        final next = was == null
            ? AppLink(
                remoteOrigin: pushed.remote.origin,
                remoteId: pushed.remote.id,
                remoteArtifactId: pushed.remoteArtifactId,
                url: pushed.url,
                baseRemoteVersion: pushed.remoteVersion,
                role: AppLink.author,
                autoPublish: autoPublish ?? false,
                pendingBundle: pending,
                published: [version],
              )
            : was.copyWith(
                remoteId: () => pushed.remote.id,
                url: pushed.url,
                baseRemoteVersion: pushed.remoteVersion,
                autoPublish: autoPublish,
                pendingBundle: () => pending,
                published: [...was.published, version],
              );
        // Who published the live code is the entry's now: an update taken
        // during the push names a newer publisher than the export saw.
        return made.copyWith(
          link: () => next,
          liveFrom: () => e?.liveFrom ?? made.liveFrom,
        );
      });
      if (await _land(aid) case final why?) return Refused(why);
      _published.add(
        PublishedVersion(
          aid: aid,
          version: version,
          remoteVersion: pushed.remoteVersion,
        ),
      );
      return Published(pushed.url, pushed.remoteVersion);
    } on _Stopped {
      return _stopped;
    } on Object catch (e) {
      _log('the publish of $aid failed: $e');
      return const Refused('The publish could not finish.');
    } finally {
      _done(aid);
    }
  }

  /// Each app's count of [stop]s, which a publish asked for before the
  /// latest one sees changed.
  final _epochs = <String, int>{};

  /// Wakes the publishes of an app waiting to try again, on [stop].
  final _stops = <String, Completer<void>>{};

  /// The outcome of a publish [stop] ended.
  static const _stopped = Refused('This publish was stopped.');

  /// Stops every publish of [aid] asked for so far, before its next push:
  /// one waiting to try again (for a connection) ends at once, one queued
  /// never starts; each ends as Refused. A push already sent still lands.
  /// Unpublish calls it, so no publish asked for before it makes the app
  /// public again.
  void stop(String aid) {
    _epochs[aid] = (_epochs[aid] ?? 0) + 1;
    _stops.remove(aid)?.complete();
  }

  /// The refusal for [e] from a publish to [host], in the person's words,
  /// the raw message under its details.
  static Refused _refused(ClaxApiError e, String host) => Refused(
    remoteErrorLine(
      e,
      host: host,
      action: 'publish apps',
      otherwise: 'The publish could not finish.',
    ),
    details: [e.message],
  );

  /// The refusal for the app itself not read here ([e], the core's).
  static Refused _unread(ClaxApiError e) =>
      Refused('Couldn’t read this app to publish it.', details: [e.message]);

  /// The name of the remote a publish goes to: [link]'s, else [rid]'s (the
  /// only connected one when null), else clax.dev.
  Future<String> _hostOf(AppLink? link, String? rid) async {
    if (link != null) return Uri.parse(link.remoteOrigin).host;
    try {
      for (final r in await remote.remotes()) {
        if (rid == null ? r.connected : r.id == rid) {
          return Uri.parse(r.origin).host;
        }
      }
    } on ClaxApiError catch (e) {
      _log('the remotes could not be read: ${e.message}');
    }
    return 'clax.dev';
  }

  /// The latest bundle version [link]'s public copy holds, from its head;
  /// null when it has none or the head cannot be read (the push then
  /// answers for the copy).
  Future<int?> _latestBundle(String aid, AppLink link) async {
    final rid = link.remoteId;
    if (rid == null) return null;
    try {
      final head = await _retrying(
        aid,
        () => remote.head(rid, link.remoteArtifactId),
      );
      return head.bundle?.bundleVersion;
    } on ClaxApiError catch (e) {
      _log('the head of ${link.remoteArtifactId} could not be read: '
          '${e.message}');
      return null;
    }
  }

  /// Publish as a new app (spec 2026-10-21-app-push-pull §5.10, §5.12):
  /// publishes local [version] of linked app [aid] (its current version
  /// when null) as a new public copy on its link's remote, and links the
  /// app to it in place of the copy it was linked to, which stays as it
  /// is. The bundle is a new lineage at version 1 with `forked_from`
  /// naming the lineage the app had ([Exporter.export]'s `newLineage`, or
  /// a fork for an installed app), and the push names the old copy as the
  /// one the new copy was made from. The new copy starts Restricted with
  /// comments off, as every first publish does; New versions keeps the
  /// link's choice.
  ///
  /// The core's link to the old copy is ended first ([RemoteApi.unlink]),
  /// so the push makes a new copy; a publish that does not land makes the
  /// core's link again, at the base it had. Runs after any publish of
  /// [aid] on its way, the core's link ended then too. Never throws.
  Future<PublishOutcome> publishAsNewApp(String aid, {int? version}) async {
    final was = (await store.entry(aid))?.link;
    if (was == null) {
      return const Refused('This app isn’t published, so it has no copy to '
          'leave.');
    }
    return _leaving(
      aid,
      was,
      version: version,
      includeData: false,
      autoPublish: was.autoPublish,
    );
  }

  /// Publish fork (spec 2026-10-21-app-push-pull §5.12): publishes local
  /// [version] (the current one when null) of [aid], an app installed from
  /// someone else's bundle, as a new public copy named [name] — the
  /// person's own app, Restricted with comments off as every first publish
  /// is. The bundle is a fork: a new lineage under this install's key,
  /// `forked_from` naming the original's lineage. With [includeData] it
  /// carries the app's documents; [autoPublish] is the new link's Publish
  /// new versions.
  ///
  /// A copy linked to the original's public copy (a recipient) publishes on
  /// that remote, the push naming the original as the copy it was made
  /// from, and is relinked to the new copy, as [publishAsNewApp] does; one
  /// installed from a file publishes on [rid] (the only connected remote
  /// when null). Never throws.
  Future<PublishOutcome> publishFork(
    String aid, {
    required String name,
    int? version,
    bool includeData = false,
    String? rid,
    bool autoPublish = false,
  }) async {
    final title = name.trim();
    if (title.isEmpty) return const Refused('Give your app a name.');
    final e = await store.entry(aid);
    if (e == null || e.role != LineageRole.installed) {
      return const Refused(
        'Only an app installed from someone else’s can be published as a '
        'fork.',
      );
    }
    final was = e.link;
    if (was != null) {
      return _leaving(
        aid,
        was,
        version: version,
        includeData: includeData,
        autoPublish: autoPublish,
        title: title,
      );
    }
    final int n;
    try {
      n = version ??
          (await exporter.api.getArtifact(aid)).artifact.currentVersion;
    } on ClaxApiError catch (e) {
      return _unread(e);
    }
    return _queue(
      aid,
      version: n,
      includeData: includeData,
      rid: rid,
      autoPublish: autoPublish,
      asNew: true,
      title: title,
    );
  }

  /// Publishes [aid], linked as [was], as a new public copy on [was]'s
  /// remote made from [was]'s copy, named [title] (the app's own when
  /// null), and links the app to it; the core's link to the old copy is
  /// ended first and made again when the publish does not land.
  Future<PublishOutcome> _leaving(
    String aid,
    AppLink was, {
    required int? version,
    required bool includeData,
    required bool autoPublish,
    String? title,
  }) async {
    final AppLink? link;
    try {
      link = await links.resolve(aid);
    } on ClaxApiError catch (e) {
      return _refused(e, Uri.parse(was.remoteOrigin).host);
    }
    final rid = link?.remoteId;
    if (link == null || rid == null) {
      return Refused(
        'This device is not signed in to ${Uri.parse(was.remoteOrigin).host}.',
      );
    }
    final int n;
    try {
      n = version ??
          (await exporter.api.getArtifact(aid)).artifact.currentVersion;
    } on ClaxApiError catch (e) {
      return _unread(e);
    }
    final out = await _queue(
      aid,
      version: n,
      includeData: includeData,
      rid: rid,
      autoPublish: autoPublish,
      asNew: true,
      title: title,
      forkedFrom: link.remoteArtifactId,
      // In the queue, after any publish of the app on its way: one still
      // waiting to try again pushes to the old copy, not a new one.
      first: () async {
        try {
          await remote.unlink(aid, rid);
        } on ClaxApiError catch (e) {
          // Not linked in the core: the push makes a new copy anyway.
          if (e.status != 404) {
            return _refused(e, Uri.parse(was.remoteOrigin).host);
          }
        }
        return null;
      },
    );
    final now = (await store.entry(aid))?.link;
    if (out is! Published && now?.remoteArtifactId == link.remoteArtifactId) {
      try {
        await remote.link(
          rid,
          aid,
          RemoteApp(
            artifactId: link.remoteArtifactId,
            title: (await store.entry(aid))?.title ?? '',
            url: link.url,
            version: link.baseRemoteVersion,
            bundleVersion: link.takenBundleVersion ?? 0,
            access: link.isAuthor ? 'editor' : 'viewer',
            general: 'restricted',
          ),
          link.baseRemoteVersion,
        );
      } on ClaxApiError catch (e) {
        _log('$aid could not be linked to ${link.remoteArtifactId} again: '
            '${e.message}');
      }
    }
    return out;
  }

  /// Finishes every upload an earlier run left pending (at launch).
  Future<void> resume() async {
    final all = await store.all();
    for (final MapEntry(key: aid, value: e) in all.entries) {
      if (e.link?.pendingBundle == null) continue;
      final why = await _land(aid);
      if (why != null) _log('the bundle of $aid was refused: $why');
    }
  }

  /// Uploads [aid]'s pending bundle until it lands: null once it has (or
  /// none is pending), else why it did not. A bundle the remote refused is
  /// dropped; one that waits for a sign-in at the link's origin is kept.
  Future<String?> _land(String aid) =>
      _landing[aid] ??= _upload(aid).whenComplete(() {
        _landing.remove(aid);
        _done(aid);
      });

  Future<String?> _upload(String aid) => _retrying(aid, () async {
        final stored = (await store.entry(aid))?.link;
        final p = stored?.pendingBundle;
        if (stored == null || p == null) return null;
        final AppLink link;
        try {
          link = (await links.resolve(aid)) ?? stored;
        } on ClaxApiError catch (e) {
          if (_transient(e)) rethrow;
          return _kept(link: stored, why: e.message);
        }
        final rid = link.remoteId;
        final Uint8List bytes;
        try {
          bytes = await _pendingFile(aid).readAsBytes();
        } on FileSystemException {
          await _drop(aid, p);
          return 'The app’s file to publish was lost.';
        }
        if (rid == null) return _kept(link: link);
        try {
          await remote.putBundle(
            aid,
            rid,
            p.remoteVersion,
            bytes,
            lineage: p.lineage,
            bundleVersion: p.bundleVersion,
            contentSha256: p.contentSha256,
          );
        } on ClaxApiError catch (e) {
          if (_transient(e)) rethrow;
          // The remote went (disconnected meanwhile): kept for the next
          // sign-in, whatever the copy would say.
          if (e.status == 404 &&
              (await links.at(link.remoteOrigin))?.id != rid) {
            return _kept(link: link);
          }
          await _drop(aid, p);
          _log('the bundle of $aid was refused: ${e.message}');
          return remoteErrorLine(
            e,
            host: Uri.parse(link.remoteOrigin).host,
            action: 'publish apps',
            otherwise: 'The app’s file couldn’t be published beside its '
                'version.',
          );
        }
        await _drop(aid, p);
        return null;
      });

  /// Why [link]'s pending bundle waits, kept for [resume] after the next
  /// sign-in at its origin.
  String _kept({required AppLink link, String? why}) {
    final host = Uri.parse(link.remoteOrigin).host;
    _log('the bundle waits for a sign-in to $host${why == null ? '' : ': $why'}');
    return 'Clax finishes publishing once this device is signed in to '
        '$host again.';
  }

  /// Drops [aid]'s pending record and its bytes when the record is still
  /// [p] (a later publish's is left as it is).
  Future<void> _drop(String aid, PendingBundle p) async {
    var mine = false;
    await store.update(aid, (e) async {
      final l = e?.link;
      final now = l?.pendingBundle;
      if (e == null ||
          l == null ||
          now == null ||
          now.remoteVersion != p.remoteVersion ||
          now.contentSha256 != p.contentSha256) {
        return null;
      }
      mine = true;
      return e.copyWith(link: () => l.copyWith(pendingBundle: () => null));
    });
    if (!mine) return;
    try {
      await _pendingFile(aid).delete();
    } on FileSystemException {
      // Gone already.
    }
  }

  /// Whether [e] is a failure for want of an answer, tried again: no
  /// connection, or a 5xx; but the core's 502 `remote_refused`,
  /// `remote_signed_out` and `remote_too_old` are the remote's final answer
  /// (a copy unpublished, a token revoked, a share removed, a hosted Clax
  /// older than the app), not a wait.
  static bool _transient(ClaxApiError e) =>
      e.status == 0 ||
      (e.status >= 500 && !_finalCodes.contains(e.code));

  static const _finalCodes = {
    'remote_refused',
    'remote_signed_out',
    'remote_too_old',
  };

  /// [call], tried again while it fails for want of an answer; with
  /// [epoch], a publish's, it throws [_Stopped] instead once [stop] was
  /// called after it was asked for.
  Future<T> _retrying<T>(
    String aid,
    Future<T> Function() call, {
    int? epoch,
  }) async {
    var wait = firstRetry;
    bool stopped() => epoch != null && (_epochs[aid] ?? 0) != epoch;
    while (true) {
      if (stopped()) throw const _Stopped();
      try {
        return await call();
      } on ClaxApiError catch (e) {
        if (!_transient(e)) rethrow;
        _log('waiting to publish $aid: ${e.message}');
        waiting.value = {...waiting.value, aid};
        await Future.any([
          clock.delay(wait),
          (_stops[aid] ??= Completer<void>()).future,
        ]);
        wait = Duration(
          microseconds: min(wait.inMicroseconds * 2, maxRetry.inMicroseconds),
        );
      }
    }
  }

  void _done(String aid) {
    if (_landing.containsKey(aid) || !waiting.value.contains(aid)) return;
    waiting.value = {...waiting.value}..remove(aid);
  }
}

/// A publish [Publisher.stop] ended.
class _Stopped implements Exception {
  const _Stopped();
}

/// What a publish pushes: the version, with the documents or not, to the
/// remote named for an app not linked yet.
typedef _Ask = ({int version, bool includeData, String? rid});

/// A publish asked for and not ended: what was asked, and its outcome.
typedef _OnTheWay = (_Ask, Future<PublishOutcome>);
