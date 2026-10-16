/// The home screen's last image, shown at once on a cold start while the
/// home app's page loads under it.
///
/// - [HomeSnapshotStore] keeps one [HomeSnapshot] in the app's cache
///   directory: an image of the home app's page and where its app tiles
///   were on it (the page's `home.tiles`).
/// - [HomeSnapshotter] takes it when the home screen is updated (a while
///   after the page's last tile report), covered or put in the background.
/// - [HomeSnapshotCover] and [HomeSnapshotView] show it until the home
///   app's page says it has drawn the home screen (`home.shown`), keeping
///   one tap, which then opens the app whose recorded tile it fell on when
///   that app is still on the home screen.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../daemon/clock.dart';
import '../ui/app_surface.dart';

/// App [id]'s tile at [rect], in CSS (logical) pixels from the page's
/// viewport's top left.
@immutable
class HomeTile {
  const HomeTile(this.id, this.rect);

  final String id;
  final Rect rect;

  Map<String, Object?> toJson() => {
    'id': id,
    'rect': {'x': rect.left, 'y': rect.top, 'w': rect.width, 'h': rect.height},
  };

  static HomeTile? fromJson(Object? j) => switch (j) {
    {
      'id': final String id,
      'rect': {
        'x': final num x,
        'y': final num y,
        'w': final num w,
        'h': final num h,
      },
    }
        when [x, y, w, h].every((v) => v.isFinite) && w >= 0 && h >= 0 =>
      HomeTile(
        id,
        Rect.fromLTWH(x.toDouble(), y.toDouble(), w.toDouble(), h.toDouble()),
      ),
    _ => null,
  };

  @override
  bool operator ==(Object other) =>
      other is HomeTile && other.id == id && other.rect == rect;

  @override
  int get hashCode => Object.hash(id, rect);
}

/// Home app [aid]'s page as it showed at [at]: [png], of a view [width] by
/// [height] logical pixels, with its app [tiles] where they were. [image]
/// is [png] decoded, once [HomeSnapshotStore.load] has.
class HomeSnapshot {
  const HomeSnapshot({
    required this.aid,
    required this.png,
    required this.width,
    required this.height,
    required this.tiles,
    required this.at,
    this.image,
  });

  final String aid;
  final Uint8List png;
  final double width, height;
  final List<HomeTile> tiles;
  final DateTime at;
  final ui.Image? image;

  /// The app whose tile holds [p] (in the snapshot's logical pixels), or
  /// null when no tile does.
  String? tileAt(Offset p) {
    for (final t in tiles.reversed) {
      if (t.rect.contains(p)) return t.id;
    }
    return null;
  }
}

/// Keeps one [HomeSnapshot] in [dir] (the app's cache directory), as one
/// file `home-snapshot.bin`: a line of JSON (`aid`, `width`, `height`,
/// `tiles`, `at`), then the PNG. It is written whole to a temporary file
/// and renamed over the last one, so a load sees one snapshot or the
/// other. A file that cannot be read whole is no snapshot.
class HomeSnapshotStore {
  HomeSnapshotStore(this.dir);

  final Directory dir;

  File get _file => File('${dir.path}/home-snapshot.bin');

  Future<void>? _writing;

  /// Keeps [s] in place of the last snapshot; saves run one at a time.
  Future<void> save(HomeSnapshot s) {
    final prior = _writing ?? Future<void>.value();
    final next = _writing = prior.then((_) => _write(s));
    return next;
  }

  Future<void> _write(HomeSnapshot s) async {
    final head = utf8.encode(
      jsonEncode({
        'aid': s.aid,
        'width': s.width,
        'height': s.height,
        'tiles': [for (final t in s.tiles) t.toJson()],
        'at': s.at.toUtc().toIso8601String(),
        'bytes': s.png.length,
      }),
    );
    final out = BytesBuilder(copy: false)
      ..add(head)
      ..addByte(0x0a)
      ..add(s.png);
    await dir.create(recursive: true);
    final tmp = File('${_file.path}.tmp');
    await tmp.writeAsBytes(out.takeBytes(), flush: true);
    await tmp.rename(_file.path);
  }

  /// Forgets the snapshot.
  Future<void> clear() async {
    try {
      await _file.delete();
    } on FileSystemException {
      // None kept.
    }
  }

  /// The kept snapshot, with its image decoded when [decode]; null when
  /// there is none or it cannot be read.
  Future<HomeSnapshot?> load({bool decode = true}) async {
    final Uint8List bytes;
    try {
      bytes = await _file.readAsBytes();
    } on FileSystemException {
      return null;
    }
    final s = parse(bytes);
    if (s == null || !decode) return s;
    try {
      final codec = await ui.instantiateImageCodec(s.png);
      final frame = await codec.getNextFrame();
      codec.dispose();
      return HomeSnapshot(
        aid: s.aid,
        png: s.png,
        width: s.width,
        height: s.height,
        tiles: s.tiles,
        at: s.at,
        image: frame.image,
      );
    } on Object {
      return null;
    }
  }

  /// The snapshot [bytes] hold (as [save] writes them), or null.
  @visibleForTesting
  static HomeSnapshot? parse(Uint8List bytes) {
    final nl = bytes.indexOf(0x0a);
    if (nl <= 0) return null;
    try {
      final j = jsonDecode(utf8.decode(bytes.sublist(0, nl)));
      if (j
          case {
            'aid': final String aid,
            'width': final num width,
            'height': final num height,
            'tiles': final List<Object?> tiles,
            'at': final String at,
            'bytes': final int length,
          }
          when width > 0 && height > 0 && length == bytes.length - nl - 1) {
        final parsed = [for (final t in tiles) HomeTile.fromJson(t)];
        if (parsed.contains(null)) return null;
        return HomeSnapshot(
          aid: aid,
          png: Uint8List.sublistView(bytes, nl + 1),
          width: width.toDouble(),
          height: height.toDouble(),
          tiles: parsed.cast<HomeTile>(),
          at: DateTime.parse(at),
        );
      }
    } on Object {
      return null;
    }
    return null;
  }
}

/// What [HomeSnapshotter.capture] gives: the home app [aid]'s page as
/// [shot], with its app [tiles] at that time.
typedef HomeCapture = ({SurfaceShot shot, String aid, List<HomeTile> tiles});

/// Takes and keeps the home screen's snapshot: [capture] gives the home
/// app's page as it shows, with the app it belongs to and its tiles (null
/// when it cannot be taken now: not in front, not drawn, covered by the
/// built-in home screen), and [store] keeps it.
///
/// [tilesChanged] takes one [settle] after the page's last tile report
/// (on [clock]), so a burst of renders makes one snapshot, when the home
/// screen is then [inFront]; [take] takes one at once (the home screen
/// being covered or put in the background). Takes run one at a time.
///
/// A snapshot keeps the tiles of the very render it shows: none is taken
/// within [quiet] of the page's last load or tile report ([loaded],
/// [tilesChanged]), and one whose [mark] (the page's loads and tile
/// reports) changed while the image was taken is dropped.
class HomeSnapshotter {
  HomeSnapshotter({
    required this.store,
    required this.capture,
    this.inFront = _always,
    this.mark = _none,
    this.clock = const SystemClock(),
    this.settle = const Duration(milliseconds: 800),
    this.quiet = const Duration(milliseconds: 500),
  });

  final HomeSnapshotStore store;
  final Future<HomeCapture?> Function() capture;
  final bool Function() inFront;
  final Object? Function() mark;
  final Duration quiet;

  static Object? _none() => null;
  DateTime? _changed;

  /// The page loaded a document.
  void loaded() => _changed = clock.now();

  static bool _always() => true;
  final Clock clock;
  final Duration settle;

  var _seq = 0;
  Future<void> _last = Future.value();
  var _disposed = false;

  /// The page reported its tiles anew.
  void tilesChanged() {
    _changed = clock.now();
    final mine = ++_seq;
    unawaited(
      clock.delay(settle).then((_) {
        if (mine == _seq && !_disposed && inFront()) return take();
      }),
    );
  }

  /// Completes once the takes asked for so far are done.
  @visibleForTesting
  Future<void> get idle => _last;

  /// Takes a snapshot now, after any in progress.
  Future<void> take() {
    final next = _last = _last.then((_) => _take());
    return next;
  }

  Future<void> _take() async {
    if (_disposed) return;
    final changed = _changed;
    if (changed != null && clock.now().difference(changed) < quiet) return;
    final before = mark();
    try {
      final c = await capture();
      if (c == null || _disposed || mark() != before) return;
      await store.save(
        HomeSnapshot(
          aid: c.aid,
          png: c.shot.png,
          width: c.shot.width,
          height: c.shot.height,
          tiles: c.tiles,
          at: clock.now(),
        ),
      );
    } on Object catch (e) {
      debugPrint('clax home: no snapshot: $e');
    }
  }

  void dispose() => _disposed = true;
}

/// The cold start's cover: [snapshot] shows ([covering]) until the home
/// app's page has drawn the home screen ([shown]) or the cover is
/// [drop]ped. The first tap on it is kept ([tap]) for [shown].
///
/// While it shows, and for [grace] after it lifts (on [clock]), it
/// [holdsActions]: a click that reached the page under it was aimed at the
/// snapshot, so a tap goes to one place, the kept tap or nowhere.
class HomeSnapshotCover extends ChangeNotifier {
  HomeSnapshotCover(
    this.snapshot, {
    this.clock = const SystemClock(),
    this.grace = const Duration(milliseconds: 300),
  });

  final HomeSnapshot snapshot;
  final Clock clock;
  final Duration grace;

  var _covering = true;
  Offset? _tap;
  DateTime? _lifted;

  /// Whether the snapshot still shows.
  bool get covering => _covering;

  /// Whether the page's own clicks are held: while the snapshot shows,
  /// and for [grace] after [shown] lifts it.
  bool get holdsActions {
    if (_covering) return true;
    final lifted = _lifted;
    return lifted != null && clock.now().difference(lifted) < grace;
  }

  /// The kept tap, in the snapshot's logical pixels.
  Offset? get queuedTap => _tap;

  /// A tap at [at] on the snapshot (its logical pixels); only the first
  /// is kept.
  void tap(Offset at) {
    if (_covering) _tap ??= at;
  }

  /// Home app [aid]'s page drew its home screen, with the apps [apps] on
  /// it: the cover goes. A kept tap on a tile the snapshot recorded opens
  /// that app with [open], when the snapshot is [aid]'s and the app is
  /// among [apps]; any other tap is dropped.
  void shown(String aid, Iterable<String> apps, void Function(String id) open) {
    if (!_covering) return;
    final tap = _tap;
    _tap = null;
    _covering = false;
    _lifted = clock.now();
    notifyListeners();
    if (tap == null || aid != snapshot.aid) return;
    final id = snapshot.tileAt(tap);
    if (id != null && apps.contains(id)) open(id);
  }

  /// The cover goes without opening anything (the built-in home screen
  /// shows instead).
  void drop() {
    if (!_covering) return;
    _tap = null;
    _covering = false;
    notifyListeners();
  }
}

/// [cover]'s snapshot, filling the width it is given from the top (in the
/// safe area, as the home app's page is), on the theme's surface. A tap
/// goes to [HomeSnapshotCover.tap] in the snapshot's pixels. [onPainted]
/// is called after its first frame.
class HomeSnapshotView extends StatefulWidget {
  const HomeSnapshotView({super.key, required this.cover, this.onPainted});

  final HomeSnapshotCover cover;
  final VoidCallback? onPainted;

  @override
  State<HomeSnapshotView> createState() => _HomeSnapshotViewState();
}

class _HomeSnapshotViewState extends State<HomeSnapshotView> {
  @override
  void initState() {
    super.initState();
    if (widget.onPainted case final painted?) {
      WidgetsBinding.instance.addPostFrameCallback((_) => painted());
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = widget.cover.snapshot;
    return ColoredBox(
      key: const ValueKey('home-snapshot'),
      color: Theme.of(context).colorScheme.surface,
      child: SafeArea(
        child: LayoutBuilder(
          builder: (context, box) {
            final scale = s.width / box.maxWidth;
            final image = s.image;
            return GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapUp: (d) => widget.cover.tap(d.localPosition * scale),
              child: ClipRect(
                child: OverflowBox(
                  alignment: Alignment.topCenter,
                  maxHeight: double.infinity,
                  child: SizedBox(
                    width: box.maxWidth,
                    height: s.height / scale,
                    child: image != null
                        ? RawImage(image: image, fit: BoxFit.fill)
                        : Image.memory(
                            s.png,
                            fit: BoxFit.fill,
                            gaplessPlayback: true,
                            errorBuilder: (_, _, _) => const SizedBox.shrink(),
                          ),
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );
  }
}
