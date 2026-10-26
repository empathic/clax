/// Exporting an app as a `.clax` bundle (bundles spec §3.1, §3.5, §5.2).
library;

import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import '../clax/api.dart';
import '../platform/bundle_key.dart';
import 'bundle_reader.dart';
import 'bundle_writer.dart';
import 'content_hash.dart';
import 'doc_snapshot.dart';
import 'installer.dart';
import 'lineage_store.dart';
import 'manifest.dart';

/// The MIME type a `.clax` file is shared as.
const bundleMime = 'application/vnd.empathic.clax+zip';

/// [title] as a file name stem (§3.1): lower-cased, runs of characters
/// outside `a-z0-9` replaced by one `-`, trimmed of `-`, cut to 40, `app`
/// when empty.
String bundleSlug(String title) {
  var s = title
      .toLowerCase()
      .replaceAll(RegExp('[^a-z0-9]+'), '-')
      .replaceAll(RegExp(r'^-+|-+$'), '');
  if (s.length > 40) s = s.substring(0, 40).replaceAll(RegExp(r'-+$'), '');
  return s.isEmpty ? 'app' : s;
}

/// What an export makes.
enum ExportMode {
  /// This install's own app (made here, or a fork): a bundle signed by
  /// this install's key, with the data switch.
  own,

  /// An installed app's kept original bundle, byte for byte (§3.5).
  reshare,

  /// A modified installed app as a new lineage under this install's key,
  /// `forked_from` naming the original (§3.5).
  fork,
}

/// What the export sheet shows for an app.
class ExportPlan {
  const ExportPlan({
    required this.aid,
    required this.title,
    required this.iconWord,
    required this.modes,
    required this.version,
    required this.fileCount,
    required this.fileBytes,
    required this.docCount,
    required this.dataAllowed,
    this.entry,
    this.modified = false,
  });

  final String aid;
  final String title;
  final String? iconWord;

  /// The choices the sheet offers, in order: `[own]` for an app of this
  /// install's, `[reshare]` for an unmodified installed app, `[reshare,
  /// fork]` for a modified one.
  final List<ExportMode> modes;

  /// The version an [ExportMode.own] export carries (§5.2); 1 for a fork;
  /// for [ExportMode.reshare], the kept bundle's ([entry]'s version).
  final int version;

  /// How many files go, and their bytes in all.
  final int fileCount;
  final int fileBytes;

  /// The app's documents at the owner's level (`<d> documents`).
  final int docCount;

  /// False past the seed caps (2,000 documents, 8 MiB): the switch is
  /// disabled with "Too much data to include".
  final bool dataAllowed;

  /// The app's lineage entry, when it has one: for a re-share, the
  /// author's name and version (`Shares <author>'s v<N>`).
  final LineageEntry? entry;

  /// For an installed app: whether it is modified (§5.4).
  final bool modified;
}

/// A built bundle: its bytes, its file name and the manifest it carries.
class ExportedBundle {
  const ExportedBundle({
    required this.bytes,
    required this.fileName,
    required this.manifest,
    this.entry,
  });

  /// The lineage entry this export makes the app's (its release recorded):
  /// written already unless the export was asked not to record it
  /// (`record: false`), when it is the caller's to write once the bundle
  /// has gone where it was going. Null for a reshare.
  final LineageEntry? entry;

  final Uint8List bytes;

  /// `<slug>.clax`.
  final String fileName;
  final Manifest manifest;
}

/// Builds bundles of the person's apps and keeps their lineages.
class Exporter {
  Exporter({
    required this.api,
    required this.store,
    required this.key,
    Random? random,
  }) : _random = random ?? Random.secure();

  final ClaxApi api;
  final LineageStore store;
  final BundleKey key;
  final Random _random;

  /// What exporting [aid] would carry, for the export sheet.
  Future<ExportPlan> plan(String aid) async {
    final d = await api.getArtifact(aid);
    final v = _current(d);
    final entry = await store.entry(aid);
    final caps = shareableCapabilities(d.artifact.capabilities);
    final state = ReleaseState(
      contentSha256: _contentHash(v),
      title: d.artifact.title,
      capabilities: caps,
    );
    final installed = entry != null && entry.role == LineageRole.installed;
    final modified =
        installed &&
        entry.isModified(
          contentSha256: state.contentSha256,
          capabilities: state.capabilities,
        );
    final (docs, fits) = await _docStats(aid);
    return ExportPlan(
      aid: aid,
      title: d.artifact.title,
      iconWord: _word(d.artifact.icon),
      modes: !installed
          ? const [ExportMode.own]
          : modified
          ? const [ExportMode.reshare, ExportMode.fork]
          : const [ExportMode.reshare],
      version: installed
          ? entry.version
          : exportVersion(_ownEntry(entry, null), state),
      fileCount: v.files.length,
      fileBytes: v.files.values.fold(0, (n, f) => n + f.size),
      docCount: docs,
      dataAllowed: fits,
      entry: entry,
      modified: modified,
    );
  }

  /// Builds [aid]'s bundle as [mode]: with the app's documents as seed
  /// when [includeData] (not for [ExportMode.reshare]), naming
  /// [authorName] as the author's, of local [version]'s files (the current
  /// version's when null). [ExportMode.own] and [ExportMode.fork] make the
  /// install's key on first use and record the release in the lineage
  /// store. Throws [BundleFailure].
  ///
  /// An app linked to a public copy this device may publish
  /// ([AppLink.isAuthor]) goes as [ExportMode.own] whoever signed its
  /// earlier releases: the public copy's lineage, signed by this install's
  /// key, at the lineage entry's version plus one (§5.8: each bundle a
  /// public copy takes must be past the last one; the number may skip),
  /// and its entry becomes this install's author entry. [atLeast] raises
  /// that floor: the bundle version is then past both the entry's version
  /// and [atLeast] (the public copy's latest bundle version).
  ///
  /// With [newLineage] an [ExportMode.own] export of an app that has a
  /// lineage goes as a new app (spec 2026-10-21-app-push-pull §5.10,
  /// Publish as a new app): a new lineage at version 1 under this
  /// install's key, `forked_from` naming the lineage it had, as a fork
  /// does, whoever made it.
  ///
  /// With [title] the bundle carries that name in place of the app's (a
  /// fork published under its own name).
  ///
  /// With [record] false nothing is written to the lineage store: the
  /// entry the export would write is [ExportedBundle.entry], for the
  /// caller to write once the bundle is accepted (a publish whose push
  /// fails leaves the lineage as it was).
  Future<ExportedBundle> export(
    String aid, {
    ExportMode mode = ExportMode.own,
    bool includeData = false,
    String? authorName,
    int? version,
    int? atLeast,
    bool record = true,
    bool newLineage = false,
    String? title,
  }) async {
    final entry = await store.entry(aid);
    if (mode == ExportMode.reshare) return _reshare(aid, entry);
    final fresh = newLineage && mode == ExportMode.own && entry != null;
    final forks = mode == ExportMode.fork || fresh;
    final linked =
        !fresh && mode == ExportMode.own && (entry?.link?.isAuthor ?? false);
    final installed =
        entry?.role == LineageRole.installed && !linked && !fresh;
    if (installed != (mode == ExportMode.fork)) {
      throw BundleFailure(
        'export',
        mode == ExportMode.fork
            ? 'Only an app installed from a bundle can be forked.'
            : 'Share this app as received, or as a new app.',
      );
    }
    final d = await api.getArtifact(aid);
    final v = version == null ? _current(d) : _version(d, version);
    final files = <String, BundleFile>{};
    // The manifest carries the declaration; clax.json, which can name the
    // person's other apps, never travels (withoutManifest).
    for (final MapEntry(key: path, value: meta)
        in withoutManifest(v.files).entries) {
      final bytes = await api.readFileBytes(aid, v.n, path);
      if (bytes == null) {
        throw BundleFailure(
          'export',
          'The app\'s file $path could not be read.',
        );
      }
      files[path] = BundleFile(bytes, type: meta.contentType);
    }
    final caps = shareableCapabilities(d.artifact.capabilities);
    final state = ReleaseState(
      contentSha256: contentHash({
        for (final MapEntry(:key, :value) in files.entries)
          key: (sha256: sha256Hex(value.bytes), size: value.bytes.length),
      }),
      title: title ?? d.artifact.title,
      capabilities: caps,
    );
    final seed = includeData ? await _seed(aid) : const <SeedDoc>[];
    final identity = await key.obtain();
    final name = (authorName == null || authorName.trim().isEmpty)
        ? null
        : authorName.trim();

    final mine = fresh
        ? null
        : linked
        ? entry!.copyWith(
            role: LineageRole.author,
            key: identity.key,
            liveFrom: () => entry.codeFrom,
          )
        : mode == ExportMode.own
        ? _ownEntry(entry, identity.key)
        : null;
    final lineage = mine?.lineage ?? newLineageId(_random);
    final release = linked
        ? max(mine!.version, atLeast ?? 0) + 1
        : exportVersion(mine, state);
    final WrittenBundle written;
    try {
      written = await BundleWriter.write(
        lineage: lineage,
        version: release,
        title: state.title,
        iconWord: _word(d.artifact.icon),
        capabilities: caps,
        files: files,
        seed: seed,
        keySeed: identity.seed,
        authorName: name,
        forkedFrom: forks
            ? ForkedFrom(
                lineage: entry!.lineage,
                version: entry.version,
                key: entry.key,
              )
            : null,
      );
    } on BundleError catch (e) {
      throw BundleFailure(
        e.code,
        'This app cannot be shared: ${e.message}.',
        details: [e.message],
      );
    }

    final base =
        mine ??
        LineageEntry(
          lineage: lineage,
          role: LineageRole.author,
          key: identity.key,
          name: name,
          version: release,
          released: const [],
          capabilities: caps,
          title: state.title,
          upstream: forks
              ? LineageUpstream(lineage: entry!.lineage, key: entry.key)
              : null,
          fromBundle: entry?.fromBundle ?? false,
          liveFrom: entry?.codeFrom,
        );
    final next = base
        .withRelease(state, version: release)
        .copyWith(name: () => name);
    if (record) await store.update(aid, (_) async => next);
    return ExportedBundle(
      bytes: written.bytes,
      fileName: '${bundleSlug(state.title)}.clax',
      manifest: written.manifest,
      entry: next,
    );
  }

  Future<ExportedBundle> _reshare(String aid, LineageEntry? entry) async {
    final kept = entry?.role == LineageRole.installed
        ? await store.keptBundle(aid)
        : null;
    if (kept == null) {
      throw const BundleFailure(
        'export',
        'The bundle this app came in is not kept here.',
      );
    }
    final Bundle b;
    try {
      b = await BundleReader.read(kept);
    } on BundleError catch (e) {
      throw BundleFailure.fromError(e);
    }
    return ExportedBundle(
      bytes: kept,
      fileName: '${bundleSlug(b.manifest.title)}.clax',
      manifest: b.manifest,
    );
  }

  /// [entry] when it is this install's own lineage under [ownKey] (any key
  /// when [ownKey] is null), else null: a new lineage is minted.
  static LineageEntry? _ownEntry(LineageEntry? entry, String? ownKey) {
    if (entry == null || entry.role != LineageRole.author) return null;
    if (ownKey != null && entry.key != ownKey) return null;
    return entry;
  }

  Future<List<SeedDoc>> _seed(String aid) async {
    final docs = <SeedDoc>[];
    var bytes = 0;
    await for (final d in ownerDocs(api, aid)) {
      docs.add(SeedDoc(d.path, d.data));
      bytes +=
          utf8.encode(jsonEncode({'path': d.path, 'data': d.data})).length + 1;
      if (docs.length > maxSeedDocs || bytes > maxSeedBytes) {
        throw const BundleFailure('too_large', 'Too much data to include.');
      }
    }
    return docs;
  }

  /// The app's document count, and whether they fit the seed caps.
  Future<(int, bool)> _docStats(String aid) async {
    final count = (await api.collections(aid))
        .where((c) => !c.path.startsWith('data/users/'))
        .fold(0, (n, c) => n + c.count);
    if (count == 0) return (0, true);
    if (count > maxSeedDocs) return (count, false);
    try {
      await _seed(aid);
      return (count, true);
    } on BundleFailure {
      return (count, false);
    }
  }

  static VersionInfo _current(ArtifactDetail d) {
    final v = d.current;
    if (v == null) {
      throw const BundleFailure('export', 'The app has no version to share.');
    }
    return v;
  }

  static VersionInfo _version(ArtifactDetail d, int n) {
    for (final v in d.versions) {
      if (v.n == n) return v;
    }
    throw BundleFailure('export', 'The app has no version $n to share.');
  }

  static String _contentHash(VersionInfo v) =>
      v.contentSha256 ??
      contentHash({
        for (final MapEntry(:key, :value) in v.files.entries)
          key: (sha256: value.sha256 ?? '', size: value.size),
      });

  static String? _word(String? icon) =>
      icon != null && RegExp(r'^[a-z][a-z0-9-]{0,31}$').hasMatch(icon)
      ? icon
      : null;
}
