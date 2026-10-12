import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// The request of the make in flight, kept so that a make cut off by the
/// app's end (the task swiped away, the process killed) can be offered
/// again at the next launch. In memory only when [file] is null.
///
/// The file holds `{"request": "<text>"}`. Writes run in the order they
/// were asked for.
class PendingMake {
  PendingMake([this.file]);

  final File? file;
  String? _memory;
  Future<void> _last = Future.value();

  /// Keeps [request] as the make in flight.
  Future<void> save(String request) => _queue(() async {
    _memory = request;
    await file?.writeAsString(jsonEncode({'request': request}), flush: true);
  });

  /// Forgets the make in flight: it ended while the app ran.
  Future<void> clear() => _queue(() async {
    _memory = null;
    final f = file;
    if (f != null && await f.exists()) await f.delete();
  });

  /// The request of a make that never ended, once: it is forgotten as it
  /// is read. Null when there is none or the file cannot be read.
  Future<String?> take() async {
    String? request;
    // One queued step, so a save queued meanwhile is not cleared by it.
    await _queue(() async {
      request = _memory;
      _memory = null;
      final f = file;
      if (f != null && await f.exists()) {
        try {
          final j = jsonDecode(await f.readAsString());
          if (j is Map && j['request'] is String) request = j['request'];
        } on FormatException {
          // A torn write: nothing to offer.
        } finally {
          await f.delete();
        }
      }
    });
    final r = request?.trim();
    return r == null || r.isEmpty ? null : r;
  }

  Future<void> _queue(Future<void> Function() op) {
    final next = _last.then((_) => op()).catchError((Object _) {});
    _last = next;
    return next;
  }
}
