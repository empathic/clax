import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import '../agent/timings.dart';
import 'clock.dart';

/// [Timings] kept as JSON lines in [file] (`<support>/timings.jsonl`), one
/// per stopped interval:
/// `{"name", "elapsed_ms", "at" (ISO 8601 UTC), "extra"}`.
///
/// Running intervals live in memory only; [started] are running from
/// the start (an interval that began before the timings were made, such
/// as at launch). Nothing here touches the file on the caller's isolate:
/// the lines the file held when the timings were made are read and parsed
/// on a background isolate (a line that cannot be read is skipped), and
/// each stopped interval is appended to it afterwards, in order, by
/// asynchronous writes ([settled] completes once they are done). [recent]
/// lists what the file held, once read, then the intervals stopped since.
/// With [announce] each line is also given to it after `clax-timing: ` (the
/// platform log, which a script reads where it cannot read the file: a
/// release Android app's), as it is stopped.
class FileTimings implements Timings {
  FileTimings(
    this.file, {
    this._clock = const SystemClock(),
    Map<String, DateTime> started = const {},
    this.announce,
  }) {
    _started.addAll(started);
    final path = file.path;
    _writes = Isolate.run(() => _read(path)).then<void>(
      (r) => _history = r,
      onError: (Object _) => _history = const [],
    );
  }

  final File file;
  final Clock _clock;
  final void Function(String line)? announce;
  final _started = <String, DateTime>{};

  /// What the file held when the timings were made, oldest first, once
  /// read.
  List<TimingRecord>? _history;

  /// The intervals stopped since, oldest first.
  final _stopped = <TimingRecord>[];

  /// The reading of the file, then each append, in order.
  late Future<void> _writes;

  /// Completes once the file has been read and every interval stopped so
  /// far is appended to it.
  Future<void> get settled => _writes;

  @override
  void start(String name) => _started[name] = _clock.now();

  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) {
    final from = _started.remove(name);
    if (from == null) return null;
    final at = _clock.now();
    final r = TimingRecord(
      name: name,
      elapsed: at.difference(from),
      at: at,
      extra: Map.unmodifiable(extra),
    );
    _stopped.add(r);
    final line = jsonEncode({
      'name': r.name,
      'elapsed_ms': r.elapsed.inMilliseconds,
      'at': r.at.toUtc().toIso8601String(),
      'extra': r.extra,
    }, toEncodable: (o) => o.toString());
    announce?.call('clax-timing: $line');
    // After the read, so the file's lines read are never this instance's.
    _writes = _writes.then((_) async {
      try {
        await file.parent.create(recursive: true);
        await file.writeAsString('$line\n', mode: FileMode.append, flush: true);
      } on FileSystemException {
        // Kept in memory for this launch only.
      }
    });
    return r;
  }

  @override
  List<TimingRecord> recent(int n) =>
      [...?_history, ..._stopped].reversed.take(n).toList();

  static List<TimingRecord> _read(String path) {
    final records = <TimingRecord>[];
    List<String> lines;
    try {
      lines = File(path).readAsLinesSync();
    } on FileSystemException {
      lines = const [];
    }
    for (final line in lines) {
      try {
        final j = (jsonDecode(line) as Map).cast<String, Object?>();
        records.add(
          TimingRecord(
            name: j['name'] as String,
            elapsed: Duration(milliseconds: (j['elapsed_ms'] as num).toInt()),
            at: DateTime.parse(j['at'] as String),
            extra: Map.unmodifiable(
              (j['extra'] as Map? ?? const {}).cast<String, Object?>(),
            ),
          ),
        );
      } on Object {
        // A broken line: skip it.
      }
    }
    return records;
  }
}
