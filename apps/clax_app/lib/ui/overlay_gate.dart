import 'dart:async';

import 'package:flutter/foundation.dart';

/// An overlay the app screen draws over an app: the composer, a permission
/// prompt ([OverlayKind.ask]) or the questions sheet.
enum OverlayKind { composer, ask, questions }

/// Decides which overlay shows, so that one shows at a time.
///
/// - The composer is never kept waiting: the person opened it.
/// - A permission prompt waits only for another prompt, and shows over the
///   composer or the questions sheet.
/// - The questions sheet waits while the composer or a prompt shows, and is
///   granted once neither does.
///
/// [request] grants an overlay at once (true) or keeps it waiting (false);
/// a waiting one is granted later and announced on [freed]. [release] takes
/// one down, or withdraws it while it waits. A shown composer or questions
/// sheet requested again is granted again; each prompt is its own request,
/// so a second one waits for the first's release.
class OverlayGate extends ChangeNotifier {
  /// The shown overlays, in the order they were granted.
  final _shown = <OverlayKind>[];

  /// The waiting requests, in the order they were made.
  final _waiting = <OverlayKind>[];
  final _freed = StreamController<OverlayKind>.broadcast();

  /// The overlay on top: the last granted of those still shown; null when
  /// none shows.
  OverlayKind? get showing => _shown.isEmpty ? null : _shown.last;

  /// Each waiting overlay as it is granted.
  Stream<OverlayKind> get freed => _freed.stream;

  bool _blocked(OverlayKind k) => switch (k) {
    OverlayKind.composer => false,
    OverlayKind.ask => _shown.contains(OverlayKind.ask),
    OverlayKind.questions =>
      _shown.contains(OverlayKind.composer) ||
          _shown.contains(OverlayKind.ask),
  };

  /// Asks to show [k]: true when it may show now; false when it waits, to
  /// be granted on [freed].
  bool request(OverlayKind k) {
    if (k != OverlayKind.ask && _shown.contains(k)) return true;
    if (_blocked(k)) {
      if (k != OverlayKind.ask && _waiting.contains(k)) return false;
      _waiting.add(k);
      return false;
    }
    _shown.add(k);
    notifyListeners();
    return true;
  }

  /// [k] was taken down (the newest shown of its kind), or, when none of
  /// its kind shows, its waiting request is withdrawn. Waiting overlays it
  /// held back are granted.
  void release(OverlayKind k) {
    final at = _shown.lastIndexOf(k);
    if (at < 0) {
      _waiting.remove(k);
      return;
    }
    _shown.removeAt(at);
    // A waiting prompt goes first: the questions sheet then waits for it.
    final granted = <OverlayKind>[];
    for (final ask in [true, false]) {
      for (var i = 0; i < _waiting.length;) {
        final w = _waiting[i];
        if ((w == OverlayKind.ask) != ask || _blocked(w)) {
          i++;
          continue;
        }
        _waiting.removeAt(i);
        _shown.add(w);
        granted.add(w);
      }
    }
    notifyListeners();
    for (final g in granted) {
      _freed.add(g);
    }
  }

  @override
  void dispose() {
    unawaited(_freed.close());
    super.dispose();
  }
}
