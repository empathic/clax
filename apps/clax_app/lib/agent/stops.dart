import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// The threads whose change the person stopped, each with the ID of its
/// newest viewer comment then ('' while that is not yet known), so the
/// worker's recovery leaves them alone until a newer comment arrives or
/// the person runs them again (`FeedbackWorker.cancelChange`,
/// `FeedbackWorker.retry`).
abstract interface class StoppedThreads {
  /// Every stopped thread, by thread ID.
  Future<Map<String, String>> all();

  /// Thread [tid] stopped at comment [commentId].
  Future<void> put(String tid, String commentId);

  Future<void> remove(String tid);
}

class MemoryStoppedThreads implements StoppedThreads {
  final _stops = <String, String>{};

  @override
  Future<Map<String, String>> all() async => Map.of(_stops);

  @override
  Future<void> put(String tid, String commentId) async =>
      _stops[tid] = commentId;

  @override
  Future<void> remove(String tid) async => _stops.remove(tid);
}

/// [StoppedThreads] kept in [file] as `{"version": 1, "stops": {tid: id}}`,
/// written whole through a temporary file and a rename; a file that does
/// not parse reads as empty.
class FileStoppedThreads implements StoppedThreads {
  FileStoppedThreads(this.file);

  final File file;
  final _memory = MemoryStoppedThreads();
  Future<void> _tail = Future.value();
  var _loaded = false;

  Future<T> _serial<T>(Future<T> Function() work) {
    final out = _tail.then((_) async {
      if (!_loaded) {
        _loaded = true;
        for (final e in (await _read()).entries) {
          await _memory.put(e.key, e.value);
        }
      }
      return work();
    });
    _tail = out.then<void>((_) {}, onError: (Object _) {});
    return out;
  }

  Future<Map<String, String>> _read() async {
    try {
      if (!await file.exists()) return const {};
      final j = jsonDecode(await file.readAsString());
      if (j case {'stops': final Map<String, Object?> stops}) {
        return {
          for (final e in stops.entries)
            if (e.value case final String id) e.key: id,
        };
      }
    } on Object {
      // Unreadable: nothing is stopped.
    }
    return const {};
  }

  Future<void> _save() async {
    await file.parent.create(recursive: true);
    final tmp = File('${file.path}.tmp');
    await tmp.writeAsString(
      jsonEncode({'version': 1, 'stops': await _memory.all()}),
      flush: true,
    );
    await tmp.rename(file.path);
  }

  @override
  Future<Map<String, String>> all() => _serial(_memory.all);

  @override
  Future<void> put(String tid, String commentId) => _serial(() async {
    await _memory.put(tid, commentId);
    await _save();
  });

  @override
  Future<void> remove(String tid) => _serial(() async {
    await _memory.remove(tid);
    await _save();
  });
}
