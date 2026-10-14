import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import '../platform/bundle_key.dart' show base32Upper;

/// Who signs a lineage on this install.
enum LineageRole {
  /// This install's key signs it: an app made here, or a fork.
  author,

  /// It came in a bundle signed by another key (or this one, installed).
  installed,
}

/// The original lineage a fork was made from.
class LineageUpstream {
  const LineageUpstream({required this.lineage, required this.key});

  final String lineage;

  /// The original's author key, base64url.
  final String key;

  Map<String, Object?> toJson() => {'lineage': lineage, 'key': key};

  static LineageUpstream? fromJson(Object? j) {
    if (j is! Map || j['lineage'] is! String || j['key'] is! String) {
      return null;
    }
    return LineageUpstream(
      lineage: j['lineage'] as String,
      key: j['key'] as String,
    );
  }
}

/// An app's release state: its live version's content hash, title and
/// capabilities (as exported, without `db.read` or `db.share`).
class ReleaseState {
  const ReleaseState({
    required this.contentSha256,
    required this.title,
    required this.capabilities,
  });

  /// `sha256:<hex>`.
  final String contentSha256;
  final String title;
  final Map<String, Object?> capabilities;
}

/// One app's lineage entry in `<support>/lineages.json`.
class LineageEntry {
  const LineageEntry({
    required this.lineage,
    required this.role,
    required this.key,
    required this.name,
    required this.version,
    required this.released,
    required this.capabilities,
    required this.title,
    this.updatesStopped = false,
    this.stoppedAt,
    this.upstream,
    this.fromBundle = false,
  });

  /// `l_` and 26 lower-case base32 characters.
  final String lineage;
  final LineageRole role;

  /// The lineage's author key, base64url.
  final String key;

  /// The author's name as the latest release carried it, or null.
  final String? name;

  /// The latest release's version here.
  final int version;

  /// Every content hash the lineage's author released that this app has
  /// carried here, the latest last.
  final List<String> released;

  /// The latest release's capabilities.
  final Map<String, Object?> capabilities;

  /// The latest release's title.
  final String title;

  /// True once the person chose "Keep mine" (`updates: stopped`).
  final bool updatesStopped;

  /// When updates were stopped.
  final DateTime? stoppedAt;

  /// The original's lineage and key, on a fork.
  final LineageUpstream? upstream;

  /// True when this app's code came from a bundle; kept through a fork.
  final bool fromBundle;

  /// §5.4: false when [contentSha256] is one of [released] and
  /// [capabilities] equal the stored ones, so a restore of an earlier
  /// release is unmodified and any other change is modified.
  bool isModified({
    required String contentSha256,
    required Map<String, Object?> capabilities,
  }) =>
      !released.contains(contentSha256) ||
      !jsonEquals(capabilities, this.capabilities);

  /// True when [state] is the latest release recorded here.
  bool isLatestRelease(ReleaseState state) =>
      released.isNotEmpty &&
      released.last == state.contentSha256 &&
      title == state.title &&
      jsonEquals(capabilities, state.capabilities);

  /// This entry after releasing [state] as [version]: its hash moves last
  /// in [released], and the title and capabilities become [state]'s.
  LineageEntry withRelease(ReleaseState state, {required int version}) =>
      copyWith(
        version: version,
        released: [
          for (final h in released)
            if (h != state.contentSha256) h,
          state.contentSha256,
        ],
        title: state.title,
        capabilities: state.capabilities,
      );

  LineageEntry copyWith({
    String? lineage,
    LineageRole? role,
    String? key,
    String? Function()? name,
    int? version,
    List<String>? released,
    Map<String, Object?>? capabilities,
    String? title,
    bool? updatesStopped,
    DateTime? Function()? stoppedAt,
    LineageUpstream? upstream,
    bool? fromBundle,
  }) => LineageEntry(
    lineage: lineage ?? this.lineage,
    role: role ?? this.role,
    key: key ?? this.key,
    name: name != null ? name() : this.name,
    version: version ?? this.version,
    released: released ?? this.released,
    capabilities: capabilities ?? this.capabilities,
    title: title ?? this.title,
    updatesStopped: updatesStopped ?? this.updatesStopped,
    stoppedAt: stoppedAt != null ? stoppedAt() : this.stoppedAt,
    upstream: upstream ?? this.upstream,
    fromBundle: fromBundle ?? this.fromBundle,
  );

  Map<String, Object?> toJson() => {
    'lineage': lineage,
    'role': role.name,
    'key': key,
    'name': name,
    'version': version,
    'released': released,
    'capabilities': capabilities,
    'title': title,
    'updates': updatesStopped ? 'stopped' : 'on',
    'stopped_at': stoppedAt?.toUtc().toIso8601String(),
    'upstream': upstream?.toJson(),
    'from_bundle': fromBundle,
  };

  /// The entry [j] holds, or null when it is not one.
  static LineageEntry? fromJson(Object? j) {
    if (j is! Map) return null;
    final role = switch (j['role']) {
      'author' => LineageRole.author,
      'installed' => LineageRole.installed,
      _ => null,
    };
    final released = j['released'];
    final caps = j['capabilities'];
    if (role == null ||
        j['lineage'] is! String ||
        j['key'] is! String ||
        j['version'] is! int ||
        j['title'] is! String ||
        released is! List ||
        caps is! Map<String, Object?>) {
      return null;
    }
    final stoppedAt = j['stopped_at'];
    return LineageEntry(
      lineage: j['lineage'] as String,
      role: role,
      key: j['key'] as String,
      name: j['name'] is String ? j['name'] as String : null,
      version: j['version'] as int,
      released: [for (final h in released) ?(h is String ? h : null)],
      capabilities: caps,
      title: j['title'] as String,
      updatesStopped: j['updates'] == 'stopped',
      stoppedAt: stoppedAt is String ? DateTime.tryParse(stoppedAt) : null,
      upstream: LineageUpstream.fromJson(j['upstream']),
      fromBundle: j['from_bundle'] == true,
    );
  }
}

/// §5.2: the version an export of [state] carries. 1 for an app with no
/// lineage yet ([entry] null); the latest version again when [state] is
/// the latest release; else the latest plus one.
int exportVersion(LineageEntry? entry, ReleaseState state) {
  if (entry == null) return 1;
  return entry.isLatestRelease(state) ? entry.version : entry.version + 1;
}

/// A new lineage ID: `l_` and 16 random bytes in lower-case RFC 4648
/// base32 without padding.
String newLineageId([Random? random]) {
  final r = random ?? Random.secure();
  final bytes = [for (var i = 0; i < 16; i++) r.nextInt(256)];
  return 'l_${base32Upper(bytes).toLowerCase()}';
}

/// The lineage of every app that was exported or installed here, in one
/// JSON file `<support>/lineages.json` keyed by artifact ID, and each
/// installed app's original bundle at `<support>/bundles/<aid>.clax`.
///
/// Each write goes to a sibling temporary file renamed into place. Reads
/// and writes are synchronous inside each call, so two calls in one
/// isolate never interleave. A missing or unreadable file is an empty
/// store; an entry that does not parse is left out.
class LineageStore {
  LineageStore(this.support, {DateTime Function()? now})
    : _now = now ?? DateTime.now;

  final Directory support;
  final DateTime Function() _now;

  File get file => File('${support.path}/lineages.json');

  /// Where [aid]'s original bundle is kept.
  File bundleFile(String aid) => File('${support.path}/bundles/$aid.clax');

  /// Every entry, by artifact ID.
  Future<Map<String, LineageEntry>> all() async => {
    for (final MapEntry(:key, :value) in _readRaw().entries)
      key: ?LineageEntry.fromJson(value),
  };

  /// [aid]'s entry, or null.
  Future<LineageEntry?> entry(String aid) async =>
      LineageEntry.fromJson(_readRaw()[aid]);

  /// Saves [entry] as [aid]'s.
  Future<void> put(String aid, LineageEntry entry) async {
    final raw = _readRaw();
    raw[aid] = entry.toJson();
    _writeRaw(raw);
  }

  /// Marks [aid]'s lineage `updates: stopped` ("Keep mine"), now.
  Future<void> stopUpdates(String aid) async {
    final e = await entry(aid);
    if (e == null) return;
    await put(aid, e.copyWith(updatesStopped: true, stoppedAt: _now));
  }

  /// Removes [aid]'s entry and its kept bundle (on `artifact_deleted`).
  Future<void> forget(String aid) async {
    final raw = _readRaw();
    if (raw.remove(aid) != null) _writeRaw(raw);
    final kept = bundleFile(aid);
    if (kept.existsSync()) kept.deleteSync();
  }

  /// Keeps [bytes] as [aid]'s original bundle, replacing any kept one.
  Future<void> keepBundle(String aid, Uint8List bytes) async {
    final target = bundleFile(aid);
    target.parent.createSync(recursive: true);
    final tmp = File('${target.path}.tmp');
    tmp.writeAsBytesSync(bytes, flush: true);
    tmp.renameSync(target.path);
  }

  /// [aid]'s kept original bundle, byte for byte, or null.
  Future<Uint8List?> keptBundle(String aid) async {
    try {
      return bundleFile(aid).readAsBytesSync();
    } on FileSystemException {
      return null;
    }
  }

  Map<String, Object?> _readRaw() {
    try {
      final j = jsonDecode(file.readAsStringSync());
      return j is Map<String, Object?> ? j : {};
    } on FileSystemException {
      return {};
    } on FormatException {
      return {};
    }
  }

  void _writeRaw(Map<String, Object?> raw) {
    support.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(jsonEncode(raw), flush: true);
    tmp.renameSync(file.path);
  }
}

/// Deep equality of two decoded JSON values.
bool jsonEquals(Object? a, Object? b) {
  if (a is Map && b is Map) {
    return a.length == b.length &&
        a.entries.every(
          (e) => b.containsKey(e.key) && jsonEquals(e.value, b[e.key]),
        );
  }
  if (a is List && b is List) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (!jsonEquals(a[i], b[i])) return false;
    }
    return true;
  }
  return a == b;
}
