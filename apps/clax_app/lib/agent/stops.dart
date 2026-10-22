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
/// not parse reads as empty. The file is small and read and written
/// synchronously, so the worker's recovery never waits on the event loop
/// for it.
class FileStoppedThreads implements StoppedThreads {
  FileStoppedThreads(this.file);

  final File file;
  Map<String, String>? _stops;

  Map<String, String> get _all => _stops ??= _read();

  Map<String, String> _read() {
    try {
      if (!file.existsSync()) return {};
      final j = jsonDecode(file.readAsStringSync());
      if (j case {'stops': final Map<String, Object?> stops}) {
        return {
          for (final e in stops.entries)
            if (e.value case final String id) e.key: id,
        };
      }
    } on Object {
      // Unreadable: nothing is stopped.
    }
    return {};
  }

  void _save() {
    file.parent.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      jsonEncode({'version': 1, 'stops': _all}),
      flush: true,
    );
    tmp.renameSync(file.path);
  }

  @override
  Future<Map<String, String>> all() async => Map.of(_all);

  @override
  Future<void> put(String tid, String commentId) async {
    _all[tid] = commentId;
    _save();
  }

  @override
  Future<void> remove(String tid) async {
    if (_all.remove(tid) != null) _save();
  }
}
