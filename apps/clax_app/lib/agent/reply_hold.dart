import 'dart:async';
import 'dart:convert';
import 'dart:io';

import '../clax/api.dart';

/// The local version of app `aid` its public copy is at now (the latest
/// local version published there), or null when none is.
typedef PublicVersion = Future<int?> Function(String aid);

/// Publishes local version `version` of app `aid` to its public copy,
/// listing `addresses` as the threads it answers: null once it is public,
/// else why it is not, in a sentence for the person. It answers as soon as
/// the publish is refused or waits for a connection, never waiting out an
/// outage; a publish that goes on by itself releases its replies when it
/// lands ([ReplyHold.release]).
typedef PublishVersion =
    Future<String?> Function(String aid, int version, List<String> addresses);

/// What a release posted: [posted] (or found posted already), [dropped]
/// (their thread is gone or no longer linked), and [failed], still held
/// for the next release. [notPublic], for Publish vN and reply, says why
/// the version is not public: then nothing was posted and every reply it
/// would have posted is held.
class ReplyRelease {
  const ReplyRelease({
    this.posted = const [],
    this.dropped = const [],
    this.failed = const [],
    this.notPublic,
  });

  final List<HeldReply> posted;
  final List<HeldReply> dropped;
  final List<HeldReply> failed;
  final String? notPublic;
}

/// An agent reply on a thread of a linked app's public copy, held until
/// local version [version], which made the change it tells of, is public;
/// [resolve] when the thread is to be resolved once the reply is posted;
/// [heldAfter] the ID of the thread's newest comment when it was held.
class HeldReply {
  const HeldReply({
    required this.artifactId,
    required this.threadId,
    required this.text,
    required this.version,
    this.resolve = false,
    this.heldAfter,
  });

  /// The reply [j] holds, or null when it is not one.
  static HeldReply? fromJson(Object? j) => switch (j) {
    {
      'artifact_id': final String aid,
      'thread_id': final String tid,
      'text': final String text,
      'version': final int version,
    } =>
      HeldReply(
        artifactId: aid,
        threadId: tid,
        text: text,
        version: version,
        resolve: j['resolve'] == true,
        heldAfter: j['held_after'] is String ? j['held_after'] as String : null,
      ),
    _ => null,
  };

  final String artifactId;
  final String threadId;
  final String text;
  final int version;
  final bool resolve;

  /// The thread's newest comment when the reply was held: an agent comment
  /// with [text] after it is this reply, posted already. Null when the
  /// thread had none or was not read, and then the reply is always posted.
  final String? heldAfter;

  Map<String, Object?> toJson() => {
    'artifact_id': artifactId,
    'thread_id': threadId,
    'text': text,
    'version': version,
    'resolve': resolve,
    'held_after': heldAfter,
  };
}

/// Where held replies are kept between runs of the app.
abstract interface class HeldReplies {
  /// Every held reply, oldest first.
  Future<List<HeldReply>> read();

  /// Keeps [replies] in place of what was kept.
  Future<void> write(List<HeldReply> replies);
}

class MemoryHeldReplies implements HeldReplies {
  var _replies = <HeldReply>[];

  @override
  Future<List<HeldReply>> read() async => List.of(_replies);

  @override
  Future<void> write(List<HeldReply> replies) async =>
      _replies = List.of(replies);
}

/// [HeldReplies] kept in [file] as `{"version": 1, "replies": [...]}`,
/// written whole through a temporary file and a rename, with the file
/// system's asynchronous calls, so the UI isolate never waits on the disk;
/// writes run one at a time, in order. A file that does not parse reads as
/// empty and is renamed aside to `<file>.bad-<n>`, never overwritten.
class FileHeldReplies implements HeldReplies {
  FileHeldReplies(this.file);

  final File file;
  Future<void> _writing = Future.value();

  @override
  Future<List<HeldReply>> read() async {
    if (!await file.exists()) return [];
    try {
      final j = jsonDecode(await file.readAsString());
      if (j case {'replies': final List<Object?> replies}) {
        return [for (final r in replies) ?HeldReply.fromJson(r)];
      }
    } on Object {
      // Not readable: set aside below.
    }
    // Kept aside, never overwritten: each reply in it is a published fix
    // whose thread has not heard back.
    var n = 1;
    while (await File('${file.path}.bad-$n').exists()) {
      n++;
    }
    await file.rename('${file.path}.bad-$n');
    return [];
  }

  @override
  Future<void> write(List<HeldReply> replies) {
    final body = jsonEncode({
      'version': 1,
      'replies': [for (final r in replies) r.toJson()],
    });
    return _writing = _writing.then((_) async {
      await file.parent.create(recursive: true);
      final tmp = File('${file.path}.tmp');
      await tmp.writeAsString(body, flush: true);
      await tmp.rename(file.path);
    });
  }
}

/// The agent's replies on a linked app's threads (spec 2026-10-21 §5.11).
///
/// A reply on a thread with a twin on a remote
/// ([ThreadView.linkedRemote]) that tells of local version N, while the
/// public copy is not at N or later ([publicVersion]), would reach the
/// remote before the fix does, so [reply] holds it ([held]), kept in
/// [store]. A reply that tells of no new version, or of one already
/// public, or on a thread with no remote twin, posts at once, as does every
/// reply when [publicVersion] is not given.
///
/// Held replies go out by [release] (after any publish of the app, with
/// the version now public: the replies for it and every earlier version)
/// or by [publishAndReply] (Publish vN and reply, which publishes N through
/// [publishVersion] first, unless the public copy is at N or later
/// already). Each is posted in turn and forgotten only once posted, or
/// once found posted already (an agent comment with its text came after
/// the comment that was the thread's newest when it was held, so a
/// release cut off by an exit posts nothing twice); its thread
/// is resolved after it when its change resolved it. A thread that is gone
/// (404) or no longer linked (`unknown_thread`, `thread_unlinked`) drops
/// its reply; any other failure keeps it held and reports it. [drop]
/// forgets the replies of a thread, or of a whole app.
///
/// Each app's operations run in a queue of its own, in call order, so a
/// reply held while a publish of its app is under way waits for it, and a
/// second Publish and reply finds the first's work done; another app's
/// replies go on meanwhile. A call made from inside an operation (from
/// [publishVersion], say) on the same app runs at once, in its place. So
/// [publishVersion] must not await work that calls the hold for its app
/// from outside the operation (another isolate's or zone's call queues
/// behind it, and waits forever).
class ReplyHold {
  ReplyHold(
    this._api, {
    this.publicVersion,
    this.publishVersion,
    HeldReplies? store,
    void Function(String line)? log,
  }) : _store = store ?? MemoryHeldReplies(),
       _log = log ?? ((_) {});

  final ClaxApi _api;

  /// The local version the public copy is at; without it no reply is
  /// held.
  final PublicVersion? publicVersion;

  /// Publishes a local version for [publishAndReply]; without it
  /// [publishAndReply] publishes nothing.
  final PublishVersion? publishVersion;

  final HeldReplies _store;
  final void Function(String) _log;
  Future<List<HeldReply>>? _loading;

  /// Each app's last operation, which its next one waits for.
  final _tails = <String, Future<void>>{};

  /// The held replies, read from the store once; a read that fails is
  /// tried again by the next call.
  Future<List<HeldReply>> _all() => _loading ??= _store.read().then(
    (r) => r,
    onError: (Object e, StackTrace st) {
      _loading = null;
      Error.throwWithStackTrace(e, st);
    },
  );

  /// Runs [f] after every operation on app [aid] queued before it.
  Future<T> _serial<T>(String aid, Future<T> Function() f) {
    if (Zone.current[#replyHold] case (final ReplyHold h, final String a)
        when identical(h, this) && a == aid) {
      return f();
    }
    final run = (_tails[aid] ?? Future<void>.value()).then(
      (_) => runZoned(f, zoneValues: {#replyHold: (this, aid)}),
    );
    final tail = run.then<void>((_) {}, onError: (Object _) {});
    _tails[aid] = tail;
    unawaited(
      tail.whenComplete(() {
        if (identical(_tails[aid], tail)) _tails.remove(aid);
      }),
    );
    return run;
  }

  /// App [aid]'s held replies, oldest first.
  Future<List<HeldReply>> held(String aid) => _serial(aid, () async => [
    for (final h in await _all())
      if (h.artifactId == aid) h,
  ]);

  /// Posts [text] as the agent's reply on thread [tid] of app [aid], or
  /// holds it when it tells of local version [version] of a thread with a
  /// twin on a remote and the public copy is not at that version yet
  /// (replacing a reply the thread held). True when it was posted, false
  /// when it was held. [resolve] is kept with a held reply; a posted
  /// reply's caller resolves the thread itself.
  Future<bool> reply(
    String aid,
    String tid,
    String text, {
    int? version,
    bool resolve = false,
  }) => _serial(aid, () async {
    if (version != null && publicVersion != null) {
      final t = (await _api.threads(
        aid,
      )).where((t) => t.id == tid).firstOrNull;
      if (t?.linkedRemote != null && !await _isPublic(aid, version)) {
        final all = await _all();
        all
          ..removeWhere((h) => h.artifactId == aid && h.threadId == tid)
          ..add(
            HeldReply(
              artifactId: aid,
              threadId: tid,
              text: text,
              version: version,
              resolve: resolve,
              heldAfter: t!.comments.lastOrNull?.id,
            ),
          );
        await _store.write(all);
        return false;
      }
    }
    await _api.replyAsAgent(aid, tid, text);
    return true;
  });

  Future<bool> _isPublic(String aid, int version) async {
    final at = await publicVersion?.call(aid);
    return at != null && version <= at;
  }

  /// Posts app [aid]'s replies held for [publicVersion] and every earlier
  /// version, now that its public copy is at [publicVersion]: what the
  /// wiring calls after any publish of the app.
  Future<ReplyRelease> release(String aid, {required int publicVersion}) =>
      _serial(aid, () => _release(aid, publicVersion));

  /// Forgets app [aid]'s held replies: those of thread [threadId] (deleted,
  /// or no longer linked), or all of them (the app unpublished, unlinked
  /// or deleted).
  Future<void> drop(String aid, {String? threadId}) => _serial(aid, () async {
    final all = await _all();
    all.removeWhere(
      (h) =>
          h.artifactId == aid && (threadId == null || h.threadId == threadId),
    );
    await _store.write(all);
  });

  /// Publish v[version] and reply: publishes local version [version] of
  /// app [aid], listing as addressed every thread whose reply waits on it
  /// or an earlier version (and [tid] when it holds none), then releases
  /// those replies and posts [text] on [tid] when [tid] held none (held
  /// for the next release when that post fails). A
  /// thread holding a reply posts the held text, never [text]; one held
  /// for a later version stays held. When the public copy is at [version]
  /// or later already, nothing is published (an older version never goes
  /// over a newer head): the held replies are only released. When the
  /// version did not become public (refused, waiting for a connection, or
  /// no [publishVersion]) nothing is posted, [text] is held for [version]
  /// on [tid] when it held none, and [ReplyRelease.notPublic] says why.
  Future<ReplyRelease> publishAndReply(
    String aid,
    String tid,
    String text, {
    required int version,
  }) => _serial(aid, () async {
    final at = await publicVersion?.call(aid);
    if (at != null && version <= at) return _release(aid, at);
    final publish = publishVersion;
    final all = await _all();
    final ownHeld = all.any((h) => h.artifactId == aid && h.threadId == tid);
    final addresses = [
      if (!ownHeld) tid,
      for (final h in all)
        if (h.artifactId == aid && h.version <= version) h.threadId,
    ];
    final why = publish == null
        ? 'Clax can’t publish this app here.'
        : await publish(aid, version, addresses);
    if (why != null) {
      if (!ownHeld) {
        all.add(
          HeldReply(
            artifactId: aid,
            threadId: tid,
            text: text,
            version: version,
            heldAfter: (await _threads(aid))
                ?.where((t) => t.id == tid)
                .firstOrNull
                ?.comments
                .lastOrNull
                ?.id,
          ),
        );
        await _store.write(all);
      }
      return ReplyRelease(notPublic: why);
    }
    final r = await _release(aid, version);
    if (ownHeld) return r;
    final threads = await _threads(aid);
    final mine = HeldReply(
      artifactId: aid,
      threadId: tid,
      text: text,
      version: version,
      heldAfter: threads
          ?.where((t) => t.id == tid)
          .firstOrNull
          ?.comments
          .lastOrNull
          ?.id,
    );
    switch (await _post(mine, threads)) {
      case _Posted.posted:
        return ReplyRelease(
          posted: [...r.posted, mine],
          dropped: r.dropped,
          failed: r.failed,
        );
      case _Posted.dropped:
        return ReplyRelease(
          posted: r.posted,
          dropped: [...r.dropped, mine],
          failed: r.failed,
        );
      case _Posted.failed:
        // Held, as a held reply whose post failed is, for the next release.
        all.add(mine);
        await _store.write(all);
        return ReplyRelease(
          posted: r.posted,
          dropped: r.dropped,
          failed: [...r.failed, mine],
        );
    }
  });

  Future<List<ThreadView>?> _threads(String aid) async {
    try {
      return await _api.threads(aid);
    } on Object catch (e) {
      _log('threads of $aid unread before posting held replies: $e');
      return null;
    }
  }

  Future<ReplyRelease> _release(String aid, int publicVersion) async {
    final all = await _all();
    final due = [
      for (final h in all)
        if (h.artifactId == aid && h.version <= publicVersion) h,
    ];
    if (due.isEmpty) return const ReplyRelease();
    final threads = await _threads(aid);
    final posted = <HeldReply>[], dropped = <HeldReply>[];
    final failed = <HeldReply>[];
    for (final h in due) {
      final outcome = await _post(h, threads);
      (switch (outcome) {
        _Posted.posted => posted,
        _Posted.dropped => dropped,
        _Posted.failed => failed,
      }).add(h);
      if (outcome != _Posted.failed) {
        all.remove(h);
        await _store.write(all);
      }
    }
    return ReplyRelease(posted: posted, dropped: dropped, failed: failed);
  }

  /// Posts [h] (and resolves its thread when it asks to), unless an agent
  /// comment with its text came after [HeldReply.heldAfter].
  Future<_Posted> _post(HeldReply h, List<ThreadView>? threads) async {
    final t = threads?.where((t) => t.id == h.threadId).firstOrNull;
    final mark = h.heldAfter == null
        ? -1
        : t?.comments.indexWhere((c) => c.id == h.heldAfter) ?? -1;
    final posted =
        mark >= 0 &&
        t!.comments.skip(mark + 1).any((c) => c.byAgent && c.body == h.text);
    if (!posted) {
      try {
        await _api.replyAsAgent(h.artifactId, h.threadId, h.text);
      } on ClaxApiError catch (e) {
        if (e.isNotFound ||
            e.code == 'unknown_thread' ||
            e.code == 'thread_unlinked') {
          _log('held reply on ${h.artifactId}/${h.threadId} dropped: $e');
          return _Posted.dropped;
        }
        _log('held reply on ${h.artifactId}/${h.threadId} failed: $e');
        return _Posted.failed;
      } on Object catch (e) {
        _log('held reply on ${h.artifactId}/${h.threadId} failed: $e');
        return _Posted.failed;
      }
    }
    if (h.resolve && t?.status != 'resolved') {
      try {
        await _api.resolveAsAgent(h.artifactId, h.threadId);
      } on Object catch (e) {
        _log('resolve of ${h.artifactId}/${h.threadId} failed: $e');
      }
    }
    return _Posted.posted;
  }
}

enum _Posted { posted, dropped, failed }
