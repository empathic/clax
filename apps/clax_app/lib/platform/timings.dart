import 'dart:convert';
import 'dart:io';

import '../agent/timings.dart';
import 'clock.dart';

/// [Timings] kept as JSON lines in [file] (`<support>/timings.jsonl`), one
/// per stopped interval:
/// `{"name", "elapsed_ms", "at" (ISO 8601 UTC), "extra"}`.
///
/// Running intervals live in memory only; [started] are running from
/// the start (an interval that began before the timings were made, such
/// as at launch). A line that cannot be read is skipped. Writes are
/// appends of one short line, made synchronously. With [announce] each
/// line is also given to it after `clax-timing: ` (the platform log, which
/// a script reads where it cannot read the file: a release Android app's).
class FileTimings implements Timings {
  FileTimings(
    this.file, {
    this._clock = const SystemClock(),
    Map<String, DateTime> started = const {},
    this.announce,
  }) {
    _started.addAll(started);
  }

  final File file;
  final Clock _clock;
  final void Function(String line)? announce;
  final _started = <String, DateTime>{};

  /// Every record, oldest first; read from [file] on first use.
  List<TimingRecord>? _records;

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
    final records = _load();
    records.add(r);
    file.parent.createSync(recursive: true);
    final line = jsonEncode({
      'name': r.name,
      'elapsed_ms': r.elapsed.inMilliseconds,
      'at': r.at.toUtc().toIso8601String(),
      'extra': r.extra,
    }, toEncodable: (o) => o.toString());
    file.writeAsStringSync('$line\n', mode: FileMode.append, flush: true);
    announce?.call('clax-timing: $line');
    return r;
  }

  @override
  List<TimingRecord> recent(int n) {
    final records = _load();
    return records.reversed.take(n).toList();
  }

  List<TimingRecord> _load() {
    final loaded = _records;
    if (loaded != null) return loaded;
    final records = <TimingRecord>[];
    List<String> lines;
    try {
      lines = file.readAsLinesSync();
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
    return _records = records;
  }
}
