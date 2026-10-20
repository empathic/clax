/// Opening a `.clax` file and installing it as a new app (bundles spec
/// §3.2, §6, §8.1).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import '../agent/icon_svg.dart';
import '../clax/api.dart';
import '../platform/bundle_key.dart';
import 'bundle_reader.dart';
import 'content_hash.dart';
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

/// A bundle's files as an install or an update publishes them: as the
/// bundle holds them, but for an `icon.svg` the icon rules refuse
/// ([iconBytesProblem]; the daemon's publish refuses it too, and a bundle
/// made before the rules, or by another client, may hold one). That icon
/// is published as [cleanIcon] writes it ([iconText]: re-read as Latin-1
/// when it is not UTF-8) when that passes the rules, and is otherwise left
/// out, so the app still installs, drawn with its word.
class PublishedFiles {
  PublishedFiles._(this.entries, this.contentSha256, this.iconChange, this._note);

  /// [b]'s files, `icon.svg` cleaned or left out as above.
  factory PublishedFiles.of(Bundle b) {
    final m = b.manifest;
    final entries = <String, FileEntry>{
      for (final MapEntry(:key, :value) in m.files.entries)
        key: FileEntry.bytes(b.files[key]!, type: value.type),
    };
    String? change;
    String? note;
    if (b.files[appIconPath] case final raw?) {
      final why = iconBytesProblem(raw);
      if (why != null) {
        final cleaned = cleanIcon(iconText(raw));
        if (cleaned != null && iconProblem(cleaned) == null) {
          entries[appIconPath] = FileEntry.bytes(
            utf8.encode(cleaned),
            type: m.files[appIconPath]!.type,
          );
          change =
              'Its icon was redrawn without what an icon may not hold: '
              '${why.message}';
          note = 'its icon was redrawn (icon rule ${why.code})';
        } else {
          entries.remove(appIconPath);
          change = 'Its icon was left out: ${why.message}';
          note = 'its icon was left out (icon rule ${why.code})';
        }
      }
    }
    final hash = change == null
        ? m.contentSha256
        : contentHash({
            for (final MapEntry(:key, :value) in entries.entries)
              key: (sha256: sha256Hex(value.bytes), size: value.bytes.length),
          });
    return PublishedFiles._(entries, hash, change, note);
  }

  /// The files to publish.
  final Map<String, FileEntry> entries;

  /// The content hash of [entries]: the bundle's, unless its icon changed.
  final String contentSha256;

  /// What became of the bundle's icon, a sentence for the person; null
  /// when it is published as the bundle holds it (or there is none).
  final String? iconChange;

  final String? _note;

  /// [note] with what became of the icon after it, for the version's note.
  String noted(String note) => _note == null ? note : '$note; $_note';
}

/// An installed app: its artifact ID, and what became of the bundle's
/// icon ([PublishedFiles.iconChange]).
class Installed {
  const Installed(this.aid, {this.iconChange});

  final String aid;
  final String? iconChange;
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
  /// ID, and what became of an icon the icon rules refuse
  /// ([PublishedFiles]); the lineage's released hashes then hold the
  /// installed files' hash before the bundle's, so the app is not taken
  /// for modified. A bundle of this install's own ([TrustDecision.mine], on
  /// `install`: the person's app deleted here and opened again) is
  /// recorded with role `author`, so later exports continue its lineage. A failure deletes the app made and throws [BundleFailure].
  ///
  /// Until the lineage entry (with `from_bundle`) is written, right after
  /// the app is made and before it is filled, a marker
  /// `<support>/installs/<lineage>.json` stands for it: [fromBundleOf]
  /// counts every app as from a bundle while one is kept, and
  /// [recoverInstalls] deletes what an install that a crash cut short
  /// made.
  ///
  /// [made] hears the new app's ID as soon as the daemon made it, before
  /// it is filled.
  Future<Installed> install(
    OpenedBundle opened, {
    void Function(String aid)? made,
  }) async {
    const separate = {
      TrustAnswer.install,
      TrustAnswer.foreign,
      TrustAnswer.forked,
    };
    if (!separate.contains(opened.decision.answer)) {
      throw BundleFailure(
        'install',
        'This file is not installed as a new app.',
        details: ['The trust decision is ${opened.decision.answer.name}.'],
      );
    }
    final m = opened.manifest;
    final b = opened.bundle;
    final who = opened.who;
    final files = PublishedFiles.of(b);
    final marker = installMarker(store, m.lineage);
    _writeMarker(marker, {'content_sha256': files.contentSha256, 'aid': null});
    final PublishResult created;
    try {
      created = await api.createArtifact(
        PublishRequest(
          title: m.title,
          icon: m.icon.word,
          capabilities: m.capabilities,
          label: 'v${m.version} from $who',
          note: files.noted('Installed v${m.version} of ${m.title} from $who'),
          entries: files.entries,
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
      _dropMarker(marker);
      throw BundleFailure(
        'install',
        'The app could not be installed.',
        details: [e.message],
      );
    } on Object {
      _dropMarker(marker);
      rethrow;
    }
    final aid = created.artifact.id;
    made?.call(aid);
    final mine =
        opened.decision.mine && opened.decision.answer == TrustAnswer.install;
    try {
      _writeMarker(marker, {'content_sha256': files.contentSha256, 'aid': aid});
      await store.put(
        aid,
        LineageEntry(
          lineage: m.lineage,
          role: mine ? LineageRole.author : LineageRole.installed,
          key: m.author.key,
          name: m.author.name,
          version: m.version,
          released: [
            if (files.contentSha256 != m.contentSha256) files.contentSha256,
            m.contentSha256,
          ],
          capabilities: m.capabilities,
          title: m.title,
          fromBundle: !mine,
        ),
      );
      final got = created.version.contentSha256;
      if (got != files.contentSha256) {
        throw BundleFailure(
          'bad_file',
          'The app could not be installed: its files changed on the way in.',
          details: [
            'The installed version is $got, the files sent '
                '${files.contentSha256}.',
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
      if (!mine) await store.keepBundle(aid, b.bytes);
      _dropMarker(marker);
      return Installed(aid, iconChange: files.iconChange);
    } on Object catch (e) {
      try {
        await api.deleteArtifact(aid);
      } on ClaxApiError {
        // Already gone.
      }
      await store.forget(aid);
      _dropMarker(marker);
      if (e is BundleFailure) rethrow;
      throw BundleFailure(
        'install',
        'The app could not be installed.',
        details: ['$e'],
      );
    }
  }

  /// Deletes what each install a crash cut short made (§8.1: a failure
  /// deletes the app), from the markers [install] keeps: the app it named,
  /// or, when it had not named one yet, every app whose live content hash
  /// is the bundle's and that has no lineage entry. Run once the daemon is
  /// up, before the person uses an app.
  Future<void> recoverInstalls() async {
    final dir = Directory('${store.support.path}/installs');
    if (!dir.existsSync()) return;
    final entries = await store.all();
    for (final f in dir.listSync()) {
      if (f is! File || !f.path.endsWith('.json')) continue;
      try {
        final j = jsonDecode(f.readAsStringSync()) as Map;
        final named = j['aid'];
        final hash = j['content_sha256'];
        final doomed = <String>[
          if (named is String) named,
          if (named == null)
            for (final a in await api.listArtifacts())
              if (!entries.containsKey(a.id) &&
                  (await liveRelease(api, a.id))?.contentSha256 == hash)
                a.id,
        ];
        for (final aid in doomed) {
          try {
            await api.deleteArtifact(aid);
          } on ClaxApiError catch (e) {
            if (!e.isNotFound) rethrow;
          }
          await store.forget(aid);
        }
        f.deleteSync();
      } on Object {
        // Kept for the next start.
      }
    }
  }
}

/// The marker [Installer.install] keeps while it installs [lineage].
File installMarker(LineageStore store, String lineage) =>
    File('${store.support.path}/installs/$lineage.json');

void _writeMarker(File f, Map<String, Object?> j) {
  f.parent.createSync(recursive: true);
  final tmp = File('${f.path}.tmp');
  final out = tmp.openSync(mode: FileMode.write);
  try {
    out.writeFromSync(utf8.encode(jsonEncode(j)));
    out.flushSync();
  } finally {
    out.closeSync();
  }
  tmp.renameSync(f.path);
}

void _dropMarker(File f) {
  if (f.existsSync()) f.deleteSync();
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
/// `from_bundle`): the agent's T2 isolation (§9) for that app. An app
/// without an entry counts as from a bundle while an install is unfinished
/// ([installMarker]), since it may be that install's.
Future<bool> Function(String aid) fromBundleOf(LineageStore store) =>
    (aid) async {
      final e = await store.entry(aid);
      if (e != null) return e.fromBundle;
      final dir = Directory('${store.support.path}/installs');
      return dir.existsSync() &&
          dir.listSync().any((f) => f.path.endsWith('.json'));
    };

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
