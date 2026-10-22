/// The embedded core as the app holds it: opened once at launch, its
/// state for the screens, the owner's key put into it, and its memory
/// trimmed on the platform's word.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'dart:io' as io show pid;

import 'package:flutter/foundation.dart';

import 'core_client.dart';
import 'ffi.dart';
import 'frame.dart';
import 'origins.dart';

/// Opens the core on [home] and keeps it for the app's life. There is no
/// restart, no backoff, no endpoint and no PID of its own: the core lives
/// in this process, and a call made while it opens waits in the core.
///
/// [open], once, before the first frame and without waiting:
///
/// 1. deletes `<support>/daemon-port` (the old app's daemon's port) and
///    the last launch's [readinessFile];
/// 2. writes `<home>/config.toml` with [sampleConfig] when it differs;
/// 3. the first time on this install (no `<support>/.cookies-cleared`),
///    clears the WebViews' cookies ([clearCookies]): the old daemon's key
///    cookies (`clax_key_<port>`) are of no use to the scheme's origins;
/// 4. opens the core ([CoreClient.open]) with [origins].
///
/// Then [state] follows the core: [CoreOpening] (where it starts), then
/// [CoreReady] or [CoreFailed], and [CoreClosed] after [close]. It never
/// goes back to opening, and a failed core stays failed. Once ready, the
/// key [apiKey] gives is put into the core (`PUT /api/sample/key`), the
/// run files ([runFiles]) are marked excluded from backup where
/// [excludeFromBackup] is given (iOS), and [readinessFile] says
/// `{"pid": <this process>, "state": "ready"}`; a failure writes
/// `{"pid", "state": "failed", "code"}` there instead, so a script that
/// launched the app knows when this launch's core answers. The same is
/// said in the platform log ([announce]: logcat on Android, where a release
/// build's files cannot be read from outside the app), as
/// `clax-core: ready pid=<pid>` or `clax-core: failed pid=<pid> code=<code>`.
///
/// The key is never logged: a refused put logs its status alone.
class CoreHost {
  CoreHost({
    required this.home,
    required this.supportDir,
    required this.origins,
    required this.apiKey,
    this._lib,
    int? pid,
    this.clearCookies,
    this.excludeFromBackup,
    void Function(String line)? log,
    void Function(String line)? announce,
  }) : _pid = pid ?? io.pid,
       _log = log ?? _devLog,
       _announce = announce ?? debugPrint;

  /// The core's home (`clax-home/` under the support directory).
  final String home;

  /// The application support directory.
  final String supportDir;

  /// The origins the WebViews load from, which the core is opened with.
  final AppOrigins origins;

  /// The owner's saved key, or null for none.
  final Future<String?> Function() apiKey;

  /// Clears every cookie of the app's WebViews; none are cleared when
  /// null.
  final Future<void> Function()? clearCookies;

  /// Marks the given paths excluded from the platform's backup (iOS);
  /// nothing is marked when null.
  final Future<void> Function(List<String> paths)? excludeFromBackup;

  final CoreLib? _lib;
  final int _pid;
  final void Function(String line) _log;
  final void Function(String line) _announce;

  static void _devLog(String line) => developer.log(line, name: 'clax core');

  /// The home's `config.toml`: `sample()` answers with Haiku 5.5 unless a
  /// page asks for the complex tier. The daily cost cap per app is the
  /// core's default ($0.50).
  static const sampleConfig = '''# Written by the Clax app at each launch.
[sample.models]
quick = "claude-haiku-5-5"
default = "claude-haiku-5-5"
complex = "claude-sonnet-5-5"
''';

  /// Where, under the support directory, this launch's core says it is
  /// ready or failed.
  static const readinessFile = 'core.json';

  /// The files under the home that each run makes again, kept out of the
  /// platform's backup.
  static const runFiles = ['net-cache', 'logs', 'core.lock', 'embed.json'];

  final _state = ValueNotifier<CoreOpen>(const CoreOpening('opening'));
  final _ready = Completer<bool>();
  CoreClient? _client;
  StreamSubscription<CoreOpen>? _states;
  Future<void> _puts = Future.value();

  /// Where the core stands.
  ValueListenable<CoreOpen> get state => _state;

  /// This isolate's calls to the core, from [open] on: a call made before
  /// the core is ready waits in the core for it.
  CoreCalls get calls =>
      _client ?? (throw StateError('the core is not opened yet'));

  /// The core's handle, for another isolate's [CoreClient.attach]; 0 when
  /// the core refused the open ([state] says why).
  int get handle => (_client ?? (throw StateError('the core is not opened yet'))).handle;

  /// Opens the core, once; later calls do nothing. Returns at once.
  void open() {
    if (_client != null) return;
    _delete(File('$supportDir/daemon-port'));
    _delete(File('$supportDir/$readinessFile'));
    Directory(home).createSync(recursive: true);
    _writeSampleConfig();
    unawaited(_clearCookiesOnce());
    final client = _client = CoreClient.open(
      home,
      origins,
      lib: _lib,
      log: _log,
    );
    _states = client.states.listen(_on, onError: (Object e) => _log('$e'));
  }

  /// Puts the saved key into the core, or clears it there when there is
  /// none, after the puts before it; waits for the core to be ready, and
  /// does nothing when it never is.
  Future<void> keyChanged() => _put(always: true);

  /// Asks the core to give back what memory it can
  /// (`POST /api/core/trim`); completes when it has answered, or could
  /// not.
  Future<void> trim() async {
    final client = _client;
    if (client == null) return;
    final a = await client.call(const CoreRequest('POST', '/api/core/trim')).answer;
    if (a case CoreAnswered(:final response) when response.status >= 300) {
      _log('the core refused a trim: ${response.status}');
    }
  }

  /// Closes the core: [state] ends at [CoreClosed].
  void close() => _client?.close();

  void _on(CoreOpen s) {
    final now = _state.value;
    // Ready, failed and closed never go back to opening; failed stays.
    if (now is CoreFailed) return;
    if (s is CoreOpening && now is! CoreOpening) return;
    _state.value = s;
    switch (s) {
      case CoreReady():
        if (!_ready.isCompleted) _ready.complete(true);
        _writeReadiness({'pid': _pid, 'state': 'ready'});
        _announce('clax-core: ready pid=$_pid');
        unawaited(_put(always: false));
        if (excludeFromBackup case final exclude?) {
          unawaited(
            exclude([for (final n in runFiles) '$home/$n']).catchError(
              (Object e) => _log('excluding the run files from backup: $e'),
            ),
          );
        }
      case CoreFailed(:final code):
        if (!_ready.isCompleted) _ready.complete(false);
        _writeReadiness({'pid': _pid, 'state': 'failed', 'code': code});
        _announce('clax-core: failed pid=$_pid code=$code');
      case CoreClosed():
        if (!_ready.isCompleted) _ready.complete(false);
        unawaited(_states?.cancel());
      case CoreOpening():
        break;
    }
  }

  /// Puts the key once the core is ready, after the puts before it. With
  /// [always] false (at open) no key puts nothing: the core starts with
  /// none.
  Future<void> _put({required bool always}) => _puts = _puts.then((_) async {
    if (!await _ready.future) return;
    final key = (await apiKey())?.trim();
    final value = key == null || key.isEmpty ? null : key;
    if (value == null && !always) return;
    final a = await calls
        .call(
          CoreRequest(
            'PUT',
            '/api/sample/key',
            headers: const [('content-type', 'application/json')],
            body: Uint8List.fromList(utf8.encode(jsonEncode({'key': value}))),
          ),
        )
        .answer;
    switch (a) {
      case CoreAnswered(:final response) when response.status != 204:
        _log('the core refused the key: ${response.status}');
      case CoreUnanswered(:final code):
        _log('the key went unanswered: $code');
      case CoreAnswered():
        break;
    }
  });

  void _writeSampleConfig() {
    final f = File('$home/config.toml');
    try {
      if (f.existsSync() && f.readAsStringSync() == sampleConfig) return;
      f.writeAsStringSync(sampleConfig);
    } on FileSystemException catch (e) {
      // The core opens with its own defaults.
      _log('writing config.toml: ${e.message}');
    }
  }

  Future<void> _clearCookiesOnce() async {
    final clear = clearCookies;
    if (clear == null) return;
    final flag = File('$supportDir/.cookies-cleared');
    if (flag.existsSync()) return;
    try {
      await clear();
      flag.writeAsStringSync('');
    } on Object catch (e) {
      _log('clearing the WebViews\' cookies: $e');
    }
  }

  void _writeReadiness(Map<String, Object?> j) {
    try {
      final tmp = File('$supportDir/$readinessFile.tmp')
        ..writeAsStringSync(jsonEncode(j), flush: true);
      tmp.renameSync('$supportDir/$readinessFile');
    } on FileSystemException catch (e) {
      _log('writing $readinessFile: ${e.message}');
    }
  }

  static void _delete(File f) {
    try {
      if (f.existsSync()) f.deleteSync();
    } on FileSystemException {
      // Left for the next launch.
    }
  }
}
