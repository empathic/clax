import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// The requests of the makes in flight, kept so that makes cut off by the
/// app's end (the task swiped away, the process killed) can be offered
/// again at the next launch. In memory only when [file] is null.
///
/// The file holds `{"requests": ["<text>", …]}`, oldest first; a file of
/// an earlier version, `{"request": "<text>"}`, is read as one request.
/// Writes run in the order they were asked for.
class PendingMake {
  PendingMake([this.file]);

  final File? file;
  final List<String> _memory = [];
  Future<void> _last = Future.value();

  /// Keeps [request] as a make in flight, beside any others.
  Future<void> save(String request) => _queue(() async {
    _memory.add(request);
    await _write();
  });

  /// Forgets the make of [request] (one of them, when several have the
  /// same request): it ended while the app ran. Without [request], forgets
  /// every make.
  Future<void> clear([String? request]) => _queue(() async {
    if (request == null) {
      _memory.clear();
    } else {
      _memory.remove(request);
    }
    await _write();
  });

  /// The request of a make that never ended, once; null when there is
  /// none. Every kept make is forgotten as it is read ([takeAll]).
  Future<String?> take() async => (await takeAll()).firstOrNull;

  /// The requests of the makes that never ended, oldest first, once: they
  /// are forgotten as they are read. Empty when there are none or the file
  /// cannot be read.
  Future<List<String>> takeAll() async {
    var requests = <String>[];
    // One queued step, so a save queued meanwhile is not cleared by it.
    await _queue(() async {
      requests = List.of(_memory);
      _memory.clear();
      final f = file;
      if (f != null && await f.exists()) {
        try {
          requests = switch (jsonDecode(await f.readAsString())) {
            {'requests': final List<Object?> rs} => [
              for (final r in rs)
                if (r is String) r,
            ],
            {'request': final String r} => [r],
            _ => <String>[],
          };
        } on FormatException {
          // A torn write: nothing to offer.
          requests = [];
        } finally {
          await f.delete();
        }
      }
    });
    return [
      for (final r in requests)
        if (r.trim().isNotEmpty) r.trim(),
    ];
  }

  Future<void> _write() async {
    final f = file;
    if (f == null) return;
    if (_memory.isEmpty) {
      if (await f.exists()) await f.delete();
      return;
    }
    await f.writeAsString(jsonEncode({'requests': _memory}), flush: true);
  }

  Future<void> _queue(Future<void> Function() op) {
    final next = _last.then((_) => op()).catchError((Object _) {});
    _last = next;
    return next;
  }
}
