import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../agent/icon_svg.dart';
import '../clax/api.dart';
import 'launcher.dart'
    show tileFace, tileScheme, EmojiFace, IconFace, MonogramFace;

/// Cleans app icons ([cleanIcon]) by artifact ID: each cleaned SVG, or
/// null for one that cannot be drawn.
typedef IconCleaner =
    Future<Map<String, String?>> Function(Map<String, String> raw);

/// [cleanIcon] for each of [raw] on a background isolate, so parsing the
/// SVGs never holds the caller's isolate; [clean] stands in for it in
/// tests.
Future<Map<String, String?>> cleanIconsAway(
  Map<String, String> raw, {
  String? Function(String raw) clean = cleanIcon,
}) => raw.isEmpty
    ? Future.value(const {})
    : Isolate.run(() => {for (final e in raw.entries) e.key: clean(e.value)});

/// [cleanIcon] for each of [raw] on this isolate: for widget tests, whose
/// fake time never hears another isolate.
Future<Map<String, String?>> cleanIconsHere(Map<String, String> raw) async => {
  for (final e in raw.entries) e.key: cleanIcon(e.value),
};

/// An app's icon as last read: the [version] it was read from and its
/// cleaned SVG ([cleanIcon]); [svg] is null when that version has none, or
/// one that cannot be drawn.
typedef IconEntry = ({int version, String? svg});

/// Where [AppIcons] keeps what it read, so the next launch paints at once.
abstract interface class IconStore {
  /// Every entry kept, by artifact ID.
  Future<Map<String, IconEntry>> loadAll();

  /// Keeps [e] as artifact [aid]'s entry.
  Future<void> save(String aid, IconEntry e);
}

/// An [IconStore] of one JSON file per app under [dir].
class FileIconStore implements IconStore {
  FileIconStore(this.dir);

  final Directory dir;

  /// Read and decoded on a background isolate.
  @override
  Future<Map<String, IconEntry>> loadAll() {
    final path = dir.path;
    return Isolate.run(() => _readAll(path));
  }

  static Map<String, IconEntry> _readAll(String path) {
    final out = <String, IconEntry>{};
    final dir = Directory(path);
    if (!dir.existsSync()) return out;
    for (final f in dir.listSync()) {
      if (f is! File || !f.path.endsWith('.json')) continue;
      try {
        final j = jsonDecode(f.readAsStringSync()) as Map<String, Object?>;
        final aid = j['aid'] as String;
        out[aid] = (version: j['version'] as int, svg: j['svg'] as String?);
      } catch (_) {
        // A torn or foreign file: read again from the daemon.
      }
    }
    return out;
  }

  @override
  Future<void> save(String aid, IconEntry e) async {
    await dir.create(recursive: true);
    final name = base64Url.encode(utf8.encode(aid)).replaceAll('=', '');
    final tmp = File('${dir.path}/$name.json.tmp');
    await tmp.writeAsString(
      jsonEncode({'aid': aid, 'version': e.version, 'svg': e.svg}),
    );
    await tmp.rename('${dir.path}/$name.json');
  }
}

/// The apps' icons (`icon.svg` of each app's live version), read once per
/// version and kept in memory and in [store].
///
/// [svgOf] answers at once with the newest icon known, which may be an
/// earlier version's until [want] has read the live one; listeners hear of
/// each change. A failed read keeps what was known and is tried again on
/// the next [want]. Every SVG is cleaned by [clean], by default on a
/// background isolate ([cleanIconsAway]).
class AppIcons extends ChangeNotifier {
  AppIcons(this.api, {this.store, this.clean = cleanIconsAway});

  final ClaxApi api;
  final IconStore? store;
  final IconCleaner clean;

  final _known = <String, IconEntry>{};
  final _reading = <String, int>{};
  Future<void>? _loaded;

  /// The newest icon known for [aid], cleaned; null when none is.
  String? svgOf(String aid) => _known[aid]?.svg;

  /// Reads what [store] keeps; [want] does it first when it was not done.
  Future<void> preload() => _loaded ??= _load();

  Future<void> _load() async {
    final s = store;
    if (s == null) return;
    try {
      final kept = await s.loadAll();
      // Cleaned again: the store is a file anyone could edit.
      final cleaned = await clean({
        for (final e in kept.entries) e.key: ?e.value.svg,
      });
      var changed = false;
      for (final e in kept.entries) {
        final have = _known[e.key];
        if (have == null || have.version < e.value.version) {
          _known[e.key] = (version: e.value.version, svg: cleaned[e.key]);
          changed = true;
        }
      }
      if (changed) notifyListeners();
    } catch (_) {
      // Nothing kept: every icon is read from the daemon.
    }
  }

  /// Makes sure [aid]'s icon at [version] is read, unless a newer one is.
  Future<void> want(String aid, int version) async {
    await preload();
    if ((_known[aid]?.version ?? 0) >= version) return;
    if ((_reading[aid] ?? 0) >= version) return;
    _reading[aid] = version;
    try {
      final raw = await api.readFile(aid, version, appIconPath);
      final svg = raw == null ? null : (await clean({aid: raw}))[aid];
      final e = (version: version, svg: svg);
      if ((_known[aid]?.version ?? 0) >= version) return;
      final changed = _known[aid]?.svg != e.svg;
      _known[aid] = e;
      if (changed) notifyListeners();
      unawaited(store?.save(aid, e).catchError((Object _) {}));
    } catch (_) {
      // Kept as it was; the next want tries again.
    } finally {
      if (_reading[aid] == version) _reading.remove(aid);
    }
  }
}

/// An app's badge: its [svg] icon clipped to a rounded square of
/// [size], or, without one (or one that fails to draw), the tile face of
/// [app] ([tileFace]) on its [tileScheme] colour.
class AppBadge extends StatelessWidget {
  const AppBadge({
    super.key,
    required this.app,
    this.svg,
    this.size = 52,
    this.radius = 16,
  });

  final ArtifactSummary app;
  final String? svg;
  final double size;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final icon = svg;
    if (icon == null) return _face(context);
    return ClipRRect(
      key: const ValueKey('app-icon'),
      borderRadius: BorderRadius.circular(radius),
      child: SvgPicture.string(
        icon,
        width: size,
        height: size,
        fit: BoxFit.cover,
        errorBuilder: (context, _, _) => _face(context),
      ),
    );
  }

  Widget _face(BuildContext context) {
    final theme = Theme.of(context);
    final tile = tileScheme(app.id, theme.brightness);
    final fg = tile.onPrimaryContainer;
    return Container(
      key: const ValueKey('app-face'),
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: tile.primaryContainer,
        borderRadius: BorderRadius.circular(radius),
      ),
      alignment: Alignment.center,
      // The face is drawn at the badge's size, never the text's, so a
      // large text scale cannot spill it out of its square.
      child: Padding(
        padding: EdgeInsets.all(size * 0.12),
        child: MediaQuery.withNoTextScaling(
          child: FittedBox(
            fit: BoxFit.scaleDown,
            child: switch (tileFace(app)) {
              EmojiFace(:final emoji) => Text(
                emoji,
                style: const TextStyle(fontSize: 28, height: 1),
              ),
              IconFace(:final icon) => Icon(icon, color: fg, size: 28),
              MonogramFace(:final letters) => Text(
                letters,
                style: theme.textTheme.titleLarge?.copyWith(
                  color: fg,
                  fontWeight: FontWeight.w600,
                  height: 1,
                ),
              ),
            },
          ),
        ),
      ),
    );
  }
}

/// [app]'s [AppBadge] with the icon [icons] knows, read for its live
/// version.
class LiveAppBadge extends StatefulWidget {
  const LiveAppBadge({
    super.key,
    required this.app,
    required this.icons,
    this.size = 52,
    this.radius = 16,
  });

  final ArtifactSummary app;
  final AppIcons icons;
  final double size;
  final double radius;

  @override
  State<LiveAppBadge> createState() => _LiveAppBadgeState();
}

class _LiveAppBadgeState extends State<LiveAppBadge> {
  @override
  void initState() {
    super.initState();
    _want();
  }

  @override
  void didUpdateWidget(LiveAppBadge old) {
    super.didUpdateWidget(old);
    if (old.app.id != widget.app.id ||
        old.app.currentVersion != widget.app.currentVersion ||
        old.icons != widget.icons) {
      _want();
    }
  }

  void _want() =>
      unawaited(widget.icons.want(widget.app.id, widget.app.currentVersion));

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.icons,
    builder: (context, _) => AppBadge(
      app: widget.app,
      svg: widget.icons.svgOf(widget.app.id),
      size: widget.size,
      radius: widget.radius,
    ),
  );
}
