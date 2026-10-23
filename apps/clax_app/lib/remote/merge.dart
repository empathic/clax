/// Combining an app's versions: the agent's merge on a draft (spec
/// 2026-10-21-app-push-pull §5.7 "Modified copy", §5.10).
library;

import 'dart:typed_data';

import '../bundle/bundle_reader.dart';
import '../bundle/installer.dart' show BundleFailure, shareableCapabilities;
import '../bundle/lineage_store.dart';
import '../bundle/updater.dart';
import '../clax/api.dart';
import '../platform/bundle_key.dart' show keyFingerprint;
import 'models.dart';
import 'publisher.dart';
import 'remote_api.dart';
import 'remote_errors.dart';

/// How a merge run ended.
sealed class MergeResult {
  const MergeResult();
}

/// The agent combined the versions: the combined [files], [title] and
/// [capabilities], and its [note] naming what it took from each. Nothing
/// is published: the caller checks them like an update first.
final class Merged extends MergeResult {
  const Merged({
    required this.files,
    required this.title,
    required this.capabilities,
    required this.note,
    this.added = noCapabilities,
  });

  final Map<String, Uint8List> files;
  final String title;
  final Map<String, Object?> capabilities;
  final String note;

  /// What [capabilities] declare beyond the person's version: the offer
  /// lists it ("Your version with vN" asks to …). [Merger] sets it once it
  /// has checked that nothing goes beyond what one of the two versions
  /// declares.
  final CapabilityDelta added;

  Merged _withAdded(CapabilityDelta added) => Merged(
    files: files,
    title: title,
    capabilities: capabilities,
    note: note,
    added: added,
  );
}

/// No capabilities added.
const noCapabilities = CapabilityDelta(capabilities: [], hosts: []);

/// [a] and [b]'s declarations together: every capability either declares,
/// and the `net` hosts of both.
Map<String, Object?> unionCapabilities(
  Map<String, Object?> a,
  Map<String, Object?> b,
) {
  List<String> hostsOf(Map<String, Object?> c) => switch (c['net']) {
    {'hosts': final List<Object?> h} => [
      for (final x in h) ?(x is String ? x : null),
    ],
    _ => const [],
  };
  final out = {...b, ...a};
  if (a.containsKey('net') || b.containsKey('net')) {
    final net = switch (a['net'] ?? b['net']) {
      final Map<String, Object?> m => m,
      _ => const <String, Object?>{},
    };
    out['net'] = {
      ...net,
      'hosts': {...hostsOf(a), ...hostsOf(b)}.toList(),
    };
  }
  return out;
}

/// The merge did not finish, for [reason], a sentence for the person:
/// the agent gave up, stopped at a limit, was cancelled ([cancelled]), or
/// could not run.
final class MergeFailed extends MergeResult {
  const MergeFailed(this.reason, {this.cancelled = false});

  final String reason;
  final bool cancelled;
}

/// Runs a merge of app `aid`'s version [mine] with [theirs] over their
/// common [base] (`AgentLoop.merge`), [from] naming the other version for
/// the agent (`v6 by @sam`), [theirsCapabilities] its capability
/// declaration (null when it is not known here), and [source] who made
/// [theirs].
typedef MergeRun =
    Future<MergeResult> Function(
      String aid, {
      required Map<String, Uint8List> mine,
      required Map<String, Uint8List> theirs,
      required Map<String, Uint8List> base,
      required String from,
      required Map<String, Object?>? theirsCapabilities,
      required VersionSource source,
      Future<void>? cancelled,
    });

/// What to do with a diverged app (spec §5.10).
enum DivergeChoice {
  /// Combine: an agent merge of the public head and the live version,
  /// published as head + 1.
  combine,

  /// Take theirs: the public head becomes the live version; the local
  /// versions after the base stay in Versions.
  takeTheirs,

  /// Publish as a new app: a new public copy and lineage, relinked.
  newApp,
}

/// How resolving a diverged app ended.
sealed class DivergeOutcome {
  const DivergeOutcome();
}

/// Done: local version [localVersion] is live (when a version was made),
/// and public version [remoteVersion] at [url] (when one was published).
final class DivergeDone extends DivergeOutcome {
  const DivergeDone({this.localVersion, this.remoteVersion, this.url});

  final int? localVersion;
  final int? remoteVersion;
  final String? url;
}

/// The local step did not land: [outcome] says how (a failed check, a
/// refusal, a cancel), the app unchanged.
final class DivergeNotTaken extends DivergeOutcome {
  const DivergeNotTaken(this.outcome);

  final UpdateOutcome outcome;
}

/// Refused or failed for [message], a sentence for the person.
final class DivergeFailed extends DivergeOutcome {
  const DivergeFailed(this.message, {this.details = const []});

  final String message;
  final List<String> details;
}

/// Combines versions of linked apps (spec §5.7 "Modified copy", §5.10).
///
/// [run] is the agent's merge (`AgentLoop.merge`); without it, or while
/// `available` answers false (no API key), nothing merges, and the sheets
/// hide Bring into mine and Combine ([canMerge]).
class Merger {
  Merger({
    required this.api,
    required this.store,
    required this.updater,
    this.publisher,
    this.run,
    bool Function()? available,
    void Function(String line)? log,
  }) : _available = available ?? (() => true),
       _log = log ?? ((_) {});

  final ClaxApi api;
  final LineageStore store;
  final Updater updater;
  /// Null without a remote client: a diverged app cannot be resolved.
  final Publisher? publisher;
  final MergeRun? run;
  final bool Function() _available;
  final void Function(String) _log;

  /// Whether an agent merge can run here now: a merge is given and the
  /// agent can run (`available`: an API key is set).
  bool get canMerge => run != null && _available();

  /// Bring vN into mine (§5.7): merges modified app [aid]'s live version
  /// with update [bundle] over the release this copy last took (its kept
  /// bundle; none when it was never kept), then updates to [bundle] with
  /// the merged files ([Updater.update]'s `merged`), so the app records
  /// release N as taken and stays modified. [confirm] is the offer "Your
  /// version with vN" of the merge's result, asked once the checks passed;
  /// false discards it. Never throws.
  Future<UpdateOutcome> bringIntoMine(
    String aid,
    Bundle bundle, {
    BundleOrigin? origin,
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
    Future<bool> Function(Merged merged)? confirm,
  }) async {
    final merge = run;
    if (merge == null || !_available()) {
      return const UpdateRefused('Combining versions needs Claude here.');
    }
    final m = bundle.manifest;
    try {
      final mine = await _liveFiles(aid);
      final mineCaps = (await api.getArtifact(aid)).artifact.capabilities;
      final base = await _keptFiles(aid);
      final account = origin?.head.bundle?.publishedBy;
      final by = account?.handle ?? m.author.name;
      final r = await merge(
        aid,
        mine: mine,
        theirs: bundle.files,
        base: base,
        from: by == null ? 'v${m.version}' : 'v${m.version} by $by',
        theirsCapabilities: m.capabilities,
        source: VersionSource(
          key: m.author.key,
          who: by ?? keyFingerprint(m.author.key) ?? m.author.key,
          origin: origin?.remoteOrigin,
          account: account,
        ),
        cancelled: cancelled,
      );
      switch (r) {
        case MergeFailed(cancelled: true):
          return const UpdateCancelled();
        case MergeFailed(:final reason):
          return UpdateRefused(
            'Claude couldn’t bring v${m.version} into yours: $reason',
          );
        case final Merged raw:
          final merged = _vet(raw, mineCaps, m.capabilities);
          if (merged == null) return const UpdateRefused(_beyondBoth);
          return await updater.update(
            aid,
            bundle,
            origin: origin,
            merged: merged,
            onStep: onStep,
            cancelled: cancelled,
            confirm: confirm == null ? null : () => confirm(merged),
          );
      }
    } on ClaxApiError catch (e) {
      return UpdateRefused('The app could not be read.', details: [e.message]);
    } on Object catch (e) {
      _log('bringing v${m.version} into $aid failed: $e');
      return UpdateRefused('The versions could not be combined.', details: [
        '$e',
      ]);
    }
  }

  /// Resolves diverged linked app [aid] as [choice] (§5.10), telling
  /// [onStep] each check step; [confirm] is asked, with the merge's result,
  /// before a combined version is published. Never throws.
  ///
  /// - [DivergeChoice.takeTheirs]: a head that carries a bundle runs the
  ///   update pipeline on it; a head made on the web has its files read
  ///   ([RemoteApi.versionFiles]) and taken as a local version
  ///   ([Updater.takeFiles]), recorded as taken (the app is then Live). The
  ///   link's base moves to the head.
  /// - [DivergeChoice.combine]: the agent merges the head's files and the
  ///   live version over the shared base's, the result is checked and
  ///   taken as a local version noting both sources, the link (here and in
  ///   the core) re-based on the head, and that version published: head + 1.
  /// - [DivergeChoice.newApp]: [Publisher.publishAsNewApp].
  Future<DivergeOutcome> resolveDiverged(
    String aid,
    DivergeChoice choice, {
    void Function(UpdateStep step)? onStep,
    Future<void>? cancelled,
    Future<bool> Function(Merged merged)? confirm,
  }) async {
    final publisher = this.publisher;
    if (publisher == null) {
      return const DivergeFailed('Publishing isn’t set up here.');
    }
    try {
      if (choice == DivergeChoice.newApp) {
        return switch (await publisher.publishAsNewApp(aid)) {
          Published(:final url, :final remoteVersion) => DivergeDone(
            remoteVersion: remoteVersion,
            url: url,
          ),
          Moved() => const DivergeFailed('The new copy could not be made.'),
          Refused(:final message, :final details) => DivergeFailed(
            message,
            details: details,
          ),
        };
      }
      final AppLink? link;
      try {
        link = await publisher.links.resolve(aid);
      } on ClaxApiError catch (e) {
        return DivergeFailed(
          'Couldn’t find where this app is published.',
          details: [e.message],
        );
      }
      if (link == null) {
        return const DivergeFailed('This app isn’t published.');
      }
      final host = Uri.parse(link.remoteOrigin).host;
      final rid = link.remoteId;
      if (rid == null) {
        return DivergeFailed('This device is not signed in to $host.');
      }
      final raid = link.remoteArtifactId;
      final RemoteHead head;
      try {
        head = await publisher.remote.head(rid, raid);
      } on ClaxApiError catch (e) {
        return _unreachable(e, host);
      }
      return switch (choice) {
        DivergeChoice.takeTheirs => await _takeTheirs(
          publisher.remote,
          aid,
          link,
          rid,
          head,
          host,
          onStep,
          cancelled,
        ),
        DivergeChoice.combine => await _combine(
          publisher,
          aid,
          link,
          rid,
          head,
          host,
          onStep,
          cancelled,
          confirm,
        ),
        DivergeChoice.newApp => throw StateError('handled above'),
      };
    } on Object catch (e) {
      _log('resolving $aid as ${choice.name} failed: $e');
      return DivergeFailed('That could not finish.', details: ['$e']);
    }
  }

  /// [m] once its capabilities are checked against the two versions'
  /// declarations, [mine] and [theirs] (null: not known here, so only
  /// [mine] counts): null when it declares anything neither does, else [m]
  /// noting what it adds beyond [mine] ([Merged.added]).
  static Merged? _vet(
    Merged m,
    Map<String, Object?> mine,
    Map<String, Object?>? theirs,
  ) {
    final both = unionCapabilities(mine, theirs ?? const {});
    if (!addedCapabilities(both, m.capabilities).isEmpty) return null;
    return m._withAdded(addedCapabilities(mine, m.capabilities));
  }

  /// The refusal of a merge that declares more than either version.
  static const _beyondBoth =
      'Claude’s combined version asks for capabilities neither version '
      'has, so Clax didn’t take it.';

  /// The failure of a read from [host] that failed with [e].
  static DivergeFailed _unreachable(ClaxApiError e, String host) =>
      DivergeFailed(_line(e, host), details: [e.message]);

  /// [e] from [host] in the person's words.
  static String _line(ClaxApiError e, String host) => remoteErrorLine(
    e,
    host: host,
    action: 'combine versions',
    otherwise: 'Couldn’t get this app’s versions from $host.',
  );

  Future<DivergeOutcome> _takeTheirs(
    RemoteApi remote,
    String aid,
    AppLink link,
    String rid,
    RemoteHead head,
    String host,
    void Function(UpdateStep)? onStep,
    Future<void>? cancelled,
  ) async {
    final raid = link.remoteArtifactId;
    final b = head.bundle;
    if (!head.webMade && b != null) {
      final Bundle bundle;
      try {
        bundle = await BundleReader.read(await remote.bundle(rid, raid, b.version));
      } on ClaxApiError catch (e) {
        return _unreachable(e, host);
      } on BundleError catch (e) {
        return DivergeFailed(BundleFailure.fromError(e).message);
      }
      final m = bundle.manifest;
      if (m.contentSha256 != b.contentSha256 ||
          m.lineage != b.lineage ||
          m.version != b.bundleVersion) {
        return DivergeFailed(
          'This app’s file isn’t the version $host shows, so Clax didn’t '
          'open it.',
        );
      }
      final out = await updater.update(
        aid,
        bundle,
        origin: BundleOrigin(
          remoteOrigin: link.remoteOrigin,
          remoteArtifactId: raid,
          remoteVersion: b.version,
          head: head,
        ),
        onStep: onStep,
        cancelled: cancelled,
      );
      if (out is! UpdateLanded) return DivergeNotTaken(out);
      final n = (await api.getArtifact(aid)).artifact.currentVersion;
      if (await _rebase(remote, aid, link, rid, head, taken: null)
          case final why?) {
        return _notRebased(n, head, host, why);
      }
      return DivergeDone(localVersion: n);
    }
    final Map<String, Uint8List> files;
    try {
      files = await remote.versionFiles(rid, raid, head.version);
    } on ClaxApiError catch (e) {
      return _unreachable(e, host);
    }
    final live = await api.getArtifact(aid);
    final caps = live.artifact.capabilities;
    final out = await updater.takeFiles(
      aid,
      entries: {
        for (final MapEntry(:key, :value) in files.entries)
          key: FileEntry.bytes(value),
      },
      title: head.title,
      capabilities: caps,
      label: 'v${head.version} from $host',
      note:
          'Took v${head.version} by ${head.publishedBy.handle} on $host; '
          'the versions made here since v${link.baseRemoteVersion} stay in '
          'Versions.',
      onStep: onStep,
      cancelled: cancelled,
    );
    if (out is! FilesTaken) return DivergeNotTaken(out);
    await _tookFrom(
      aid,
      VersionSource.account(link.remoteOrigin, head.publishedBy),
    );
    final why = await _rebase(
      remote,
      aid,
      link,
      rid,
      head,
      taken: ReleaseState(
        contentSha256: out.contentSha256,
        title: head.title,
        capabilities: shareableCapabilities(caps),
      ),
    );
    if (why != null) return _notRebased(out.version, head, host, why);
    return DivergeDone(localVersion: out.version);
  }

  Future<DivergeOutcome> _combine(
    Publisher publisher,
    String aid,
    AppLink link,
    String rid,
    RemoteHead head,
    String host,
    void Function(UpdateStep)? onStep,
    Future<void>? cancelled,
    Future<bool> Function(Merged merged)? confirm,
  ) async {
    final merge = run;
    if (merge == null || !_available()) {
      return const DivergeFailed('Combining versions needs Claude here.');
    }
    final remote = publisher.remote;
    final raid = link.remoteArtifactId;
    final Map<String, Uint8List> theirs, base;
    try {
      theirs = await remote.versionFiles(rid, raid, head.version);
      base = await remote.versionFiles(rid, raid, link.baseRemoteVersion);
    } on ClaxApiError catch (e) {
      return _unreachable(e, host);
    }
    final live = await api.getArtifact(aid);
    final here = live.artifact.currentVersion;
    final mineCaps = live.artifact.capabilities;
    final mine = await _liveFiles(aid);
    final who = head.publishedBy.handle;
    final r = await merge(
      aid,
      mine: mine,
      theirs: theirs,
      base: base,
      from: 'v${head.version} by $who on $host',
      theirsCapabilities: head.capabilities,
      source: VersionSource.account(link.remoteOrigin, head.publishedBy),
      cancelled: cancelled,
    );
    final Merged merged;
    switch (r) {
      case MergeFailed(cancelled: true):
        return const DivergeNotTaken(UpdateCancelled());
      case MergeFailed(:final reason):
        return DivergeFailed('Claude couldn’t combine them: $reason');
      case final Merged m:
        final vetted = _vet(m, mineCaps, head.capabilities);
        if (vetted == null) return const DivergeFailed(_beyondBoth);
        merged = vetted;
    }
    final out = await updater.takeFiles(
      aid,
      entries: {
        for (final MapEntry(:key, :value) in merged.files.entries)
          key: FileEntry.bytes(value),
      },
      title: merged.title,
      capabilities: merged.capabilities,
      label: 'v${head.version} and v$here combined',
      note:
          'From v${head.version} by $who on $host and v$here here: '
          '${merged.note}',
      onStep: onStep,
      cancelled: cancelled,
      confirm: confirm == null ? null : () => confirm(merged),
    );
    if (out is! FilesTaken) return DivergeNotTaken(out);
    await _tookFrom(
      aid,
      VersionSource.account(link.remoteOrigin, head.publishedBy),
    );
    if (await _rebase(remote, aid, link, rid, head, taken: null)
        case final why?) {
      return _notRebased(out.version, head, host, why);
    }
    return switch (await publisher.publish(aid, version: out.version)) {
      Published(:final url, :final remoteVersion) => DivergeDone(
        localVersion: out.version,
        remoteVersion: remoteVersion,
        url: url,
      ),
      Moved() => DivergeFailed(
        'v${out.version} is here, but $host moved on again before it was '
        'published.',
      ),
      Refused(:final message, :final details) => DivergeFailed(
        'v${out.version} is here, but it isn’t public: $message',
        details: details,
      ),
    };
  }

  /// Records [from] as who published the code now live in [aid]
  /// (`LineageEntry.liveFrom`), for the person's trust before Claude next
  /// reads the app.
  Future<void> _tookFrom(String aid, VersionSource from) =>
      store.update(aid, (e) async => e?.copyWith(liveFrom: () => from));

  /// Moves [aid]'s link base to [head] in the core (a link made again at
  /// the head's version) and then here, recording [taken] as a release when
  /// given (files taken from a web-made head). Answers the core's refusal,
  /// or null once both moved: when the core refuses, neither base moves.
  Future<ClaxApiError?> _rebase(
    RemoteApi remote,
    String aid,
    AppLink link,
    String rid,
    RemoteHead head, {
    required ReleaseState? taken,
  }) async {
    try {
      await remote.link(
        rid,
        aid,
        RemoteApp(
          artifactId: link.remoteArtifactId,
          title: head.title,
          url: head.url,
          version: head.version,
          bundleVersion: head.bundle?.bundleVersion ?? 0,
          access: link.isAuthor ? 'editor' : 'viewer',
          general: 'restricted',
        ),
        head.version,
      );
    } on ClaxApiError catch (e) {
      _log('the core could not re-base $aid on v${head.version}: '
          '${e.message}');
      return e;
    }
    await store.update(aid, (e) async {
      final l = e?.link;
      if (e == null || l == null) return e;
      final moved = e.copyWith(
        link: () => l.copyWith(baseRemoteVersion: head.version),
      );
      return taken == null
          ? moved
          : moved.withRelease(taken, version: moved.version);
    });
    return null;
  }

  /// The failure of a resolve whose local version [n] landed but whose
  /// link could not be re-based on [head] (the core's refusal [e]): the
  /// app keeps [n], its link stays on the old base, and nothing was
  /// published.
  static DivergeFailed _notRebased(
    int n,
    RemoteHead head,
    String host,
    ClaxApiError e,
  ) => DivergeFailed(
    'v$n is here, but Clax couldn’t record it as based on $host’s '
    'v${head.version}, so nothing was published.',
    details: [e.message],
  );

  /// [aid]'s live version's files.
  Future<Map<String, Uint8List>> _liveFiles(String aid) async {
    final d = await api.getArtifact(aid);
    final n = d.artifact.currentVersion;
    final out = <String, Uint8List>{};
    for (final path in d.current?.files.keys ?? const <String>[]) {
      final bytes = await api.readFileBytes(aid, n, path);
      if (bytes != null) out[path] = bytes;
    }
    return out;
  }

  /// The files of the release [aid] last took (its kept bundle), or none.
  Future<Map<String, Uint8List>> _keptFiles(String aid) async {
    final kept = await store.keptBundle(aid);
    if (kept == null) return const {};
    try {
      return (await BundleReader.read(kept)).files;
    } on BundleError catch (e) {
      _log('the kept bundle of $aid could not be read: $e');
      return const {};
    }
  }
}
