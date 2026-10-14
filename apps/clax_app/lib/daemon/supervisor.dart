import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';
import 'dart:io' as io show pid;
import 'dart:math' as math;

import 'clock.dart';
import 'exe_locator.dart';
import 'health.dart';
import 'ports.dart';
import 'process.dart';

/// What the embedded daemon is doing.
sealed class DaemonState {
  const DaemonState();
}

/// A daemon is being started (or the supervisor has not started one yet).
final class Starting extends DaemonState {
  const Starting();
  @override
  String toString() => 'Starting()';
}

/// A daemon answers at [endpoint].
final class Up extends DaemonState {
  const Up(this.endpoint);
  final DaemonEndpoint endpoint;
  @override
  String toString() => 'Up($endpoint)';
}

/// The daemon exited (or failed to start); it is started again after
/// [after]. [attempt] counts the exits since the daemon last stayed up for
/// a minute.
final class Restarting extends DaemonState {
  const Restarting(this.attempt, this.after);
  final int attempt;
  final Duration after;
  @override
  String toString() => 'Restarting($attempt, $after)';
}

/// The daemon exited too often, or did not come up; nothing restarts it
/// until `ensureUp` or `onResume`. [stderrTail] is the last lines the
/// daemon wrote, to stdout (its log) and stderr, as they arrived, across
/// its runs.
final class Failed extends DaemonState {
  const Failed(this.reason, this.stderrTail);
  final String reason;
  final List<String> stderrTail;
  @override
  String toString() => 'Failed($reason)';
}

/// Where a running daemon answers, and the bearer token its write routes
/// require.
class DaemonEndpoint {
  const DaemonEndpoint({
    required this.base,
    required this.token,
    required this.pid,
    required this.port,
  });
  final Uri base;
  final String token;
  final int pid;
  final int port;
  @override
  String toString() => 'DaemonEndpoint($base, pid $pid)';
}

/// The key cookie for a daemon started with `--require-credential` at
/// [base] with [token]: `clax_key_<port>`, holding the token, for the
/// daemon's host and every path. With it in a WebView's cookie store, the
/// shell there passes the daemon's credential gate and fetches the token
/// (`GET /api/token`) as on a desktop. Pages on an artifact's own origin
/// (`<id>.localhost`) never receive it.
({String name, String value, String domain, String path}) keyCookie(
  Uri base,
  String token,
) =>
    (name: 'clax_key_${base.port}', value: token, domain: base.host, path: '/');

/// `ensureUp` could not bring the daemon up: it reached [Failed].
class DaemonFailure implements Exception {
  const DaemonFailure(this.reason, this.stderrTail);
  final String reason;
  final List<String> stderrTail;
  @override
  String toString() => 'DaemonFailure: $reason';
}

/// `ensureUp` was waiting when `stop` was called.
class DaemonStopped implements Exception {
  const DaemonStopped();
  @override
  String toString() => 'DaemonStopped';
}

/// Starts, watches and restarts the embedded `clax` daemon.
///
/// The daemon runs as `<exe> --port <P> serve --foreground` with a clean
/// environment (`CLAX_HOME` = [home], `HOME` = the support directory,
/// `TMPDIR` = the cache directory, a system `PATH`, `RUST_LOG=warn`, and
/// `ANTHROPIC_API_KEY` when `apiKey` gives one, read at each start so pages'
/// `sample()` calls spend the owner's key) and [home] as its working
/// directory. Before each start `<home>/config.toml` is written with
/// [sampleConfig] (the app's model choices for `sample()`). The key is never
/// logged: it is in no state, reason or message the supervisor gives. Before each start, a daemon that
/// `<home>/daemon.json` names is stopped (SIGTERM, then SIGKILL after 3 s)
/// when its PID is alive and `/healthz` on its port answers with that PID;
/// a process that does not is left alone, since its PID may have been
/// reused. The port asked for is the one kept in
/// `<support>/daemon-port` when it binds, so the WebView's origin stays the
/// same; the daemon is up once `daemon.json` names the started PID and
/// `/healthz` on the port it reports answers, within 15 s. The endpoint's
/// port and token come from `daemon.json`, and that port is kept.
///
/// An exit the supervisor did not ask for (or a failed start) restarts the
/// daemon after [backoff] (5 s from the fifth on); five within sixty
/// seconds, or [startFailAfter] starts in a row that never come up, is
/// [Failed]. After [stop] nothing restarts until [ensureUp].
class DaemonSupervisor {
  DaemonSupervisor({
    required this._exe,
    required this._home,
    required String supportDir,
    required String cacheDir,
    required this._starter,
    required this._ports,
    required this._health,
    required this._pids,
    this._clock = const SystemClock(),
    this._android = false,
    this._apiKey,
    this._requireCredential = false,
  }) : _support = supportDir,
       _cache = cacheDir;

  /// The variable the daemon reads the key from (`[sample] api_key_env`).
  static const keyEnv = 'ANTHROPIC_API_KEY';

  /// The home's `config.toml`: `sample()` answers with Haiku 5.5 unless a
  /// page asks for the complex tier. The daily cost cap per app is the
  /// daemon's default ($0.50).
  static const sampleConfig = '''# Written by the Clax app at each daemon start.
[sample.models]
quick = "claude-haiku-5-5"
default = "claude-haiku-5-5"
complex = "claude-sonnet-5-5"
''';

  static const backoff = [
    Duration(milliseconds: 200),
    Duration(milliseconds: 500),
    Duration(seconds: 1),
    Duration(seconds: 2),
    Duration(seconds: 5),
  ];

  /// How long a start may take to answer `/healthz`.
  static const healthLimit = Duration(seconds: 15);

  /// How often a starting daemon is probed.
  static const pollInterval = Duration(milliseconds: 50);

  /// How long a daemon gets after SIGTERM before SIGKILL.
  static const termGrace = Duration(seconds: 3);

  /// Exits within this window count toward [failAfter]; a daemon that
  /// stays up this long resets the backoff.
  static const failWindow = Duration(seconds: 60);
  static const failAfter = 5;

  /// Starts in a row that never come up (exit, no answer within
  /// [healthLimit], or no process) before [Failed].
  static const startFailAfter = 3;
  static const tailLines = 50;

  final ExeLocator _exe;
  final String _home;
  final String _support;
  final String _cache;
  final ProcessStarter _starter;
  final PortPicker _ports;
  final HealthProbe _health;
  final PidKiller _pids;
  final Clock _clock;
  final bool _android;
  final Future<String?> Function()? _apiKey;

  /// Starts the daemon with `--require-credential`: it answers nothing but
  /// `/healthz`, the web UI's static files and page content by artifact ID
  /// without the token or a cookie for it, `GET /api/token` included, so
  /// other programs on the device's shared loopback cannot read the apps or
  /// fetch the token. A WebView reaches it with [keyCookie] set.
  final bool _requireCredential;

  /// [restart] stopped the running daemon: its exit starts the next one at
  /// once, and is not counted as a failure.
  bool _restartAsked = false;

  final _states = StreamController<DaemonState>.broadcast();
  DaemonState _state = const Starting();

  final _tail = ListQueue<String>();
  final _exits = <DateTime>[];
  int _attempt = 0;
  int _failedStarts = 0;
  String _lastReason = 'unknown';

  /// The loop is running (between `ensureUp` and its end on stop or
  /// failure).
  Completer<void>? _loopDone;
  bool _stopping = false;
  Future<void>? _stopFuture;
  RunningProcess? _proc;
  Completer<DaemonEndpoint>? _waiter;
  Completer<void>? _wake;
  bool _skipBackoff = false;

  /// [stop] was called and no `ensureUp` since.
  bool _stopped = false;

  Stream<DaemonState> get states => _states.stream;
  DaemonState get state => _state;

  /// The running daemon's endpoint, starting it (or waiting for the start
  /// or restart under way) when there is none. Throws [DaemonFailure] when
  /// the daemon reaches [Failed], and [DaemonStopped] when [stop] is called
  /// first. After [Failed] or [stop], it starts afresh.
  Future<DaemonEndpoint> ensureUp() {
    final stopping = _stopFuture;
    if (stopping != null) return stopping.then((_) => ensureUp());
    _stopped = false;
    final s = _state;
    if (_loopDone != null && s is Up) return Future.value(s.endpoint);
    final w = _waiter ??= Completer<DaemonEndpoint>();
    if (_loopDone == null) {
      _exits.clear();
      _attempt = 0;
      _failedStarts = 0;
      unawaited(_loop());
    }
    return w.future;
  }

  /// Called when the app comes back to the foreground: a daemon that does
  /// not answer `/healthz` twice in a row is killed and started again at
  /// once, a pending backoff is cut short, and a [Failed] supervisor
  /// starts afresh (exits while the app was in the background, such as
  /// Android's phantom-process kills, are forgotten). Does nothing before
  /// the first start or after [stop].
  Future<void> onResume() async {
    if (_stopped || _stopping) return;
    if (_loopDone == null) {
      if (_state is Failed) {
        unawaited(ensureUp().then((_) {}, onError: (Object _) {}));
      }
      return;
    }
    final s = _state;
    if (s is Restarting) {
      _wakeUp();
      return;
    }
    if (s is! Up) return;
    if (await _health.up(s.endpoint.base)) return;
    if (!identical(_state, s) || _stopping) return;
    if (await _health.up(s.endpoint.base)) return;
    if (!identical(_state, s) || _stopping) return;
    _lastReason = 'did not answer /healthz on resume';
    _skipBackoff = true;
    _proc?.kill(ProcessSignal.sigkill);
  }

  /// Stops the running daemon (SIGTERM, then SIGKILL after 3 s) and starts
  /// it again at once, so it reads the environment afresh (a changed API
  /// key). The exit counts toward no failure. Does nothing when no daemon
  /// process is running; the next start reads the key anyway.
  Future<void> restart() async {
    final p = _proc;
    if (p == null || _stopping || _loopDone == null) return;
    _restartAsked = true;
    await _terminate(p);
  }

  /// Stops the daemon (SIGTERM, then SIGKILL after 3 s) and does not
  /// restart it. Completes once it has exited.
  Future<void> stop() =>
      _stopFuture ??= _stop().whenComplete(() => _stopFuture = null);

  Future<void> _stop() async {
    _stopped = true;
    final loop = _loopDone;
    if (loop == null) return;
    _stopping = true;
    _wakeUp();
    final p = _proc;
    if (p != null) await _terminate(p);
    await loop.future;
  }

  Future<void> _loop() async {
    final done = _loopDone = Completer<void>();
    _stopping = false;
    try {
      while (!_stopping) {
        _set(const Starting());
        final started = await _startOnce();
        if (_stopping) break;
        if (started != null) {
          final (proc, endpoint) = started;
          final upAt = _clock.now();
          _failedStarts = 0;
          _writePort(endpoint.port);
          _set(Up(endpoint));
          final w = _waiter;
          _waiter = null;
          w?.complete(endpoint);
          final code = await proc.exitCode;
          _proc = null;
          _forgetDaemonJson(proc.pid);
          if (_stopping) break;
          if (_restartAsked) {
            _restartAsked = false;
            continue;
          }
          // A kill by onResume keeps the reason it gave.
          if (!_skipBackoff) _lastReason = 'exited with code $code';
          if (_clock.now().difference(upAt) >= failWindow) _attempt = 0;
        }
        final now = _clock.now();
        _exits
          ..add(now)
          ..removeWhere((t) => now.difference(t) >= failWindow);
        _attempt++;
        if (started == null) _failedStarts++;
        final String? failure;
        if (_failedStarts >= startFailAfter) {
          failure =
              'clax did not come up $_failedStarts times in a row; last: $_lastReason';
        } else if (_exits.length >= failAfter) {
          failure =
              'clax stopped ${_exits.length} times within a minute; last: $_lastReason';
        } else {
          failure = null;
        }
        if (failure != null) {
          final reason = failure;
          final tail = List<String>.unmodifiable(_tail);
          _set(Failed(reason, tail));
          final w = _waiter;
          _waiter = null;
          w?.completeError(DaemonFailure(reason, tail));
          break;
        }
        final skip = _skipBackoff;
        _skipBackoff = false;
        final after = skip
            ? Duration.zero
            : backoff[math.min(_attempt - 1, backoff.length - 1)];
        _set(Restarting(_attempt, after));
        if (!skip) await _sleep(after);
      }
    } finally {
      _loopDone = null;
      final w = _waiter;
      if (_stopping && w != null) {
        _waiter = null;
        w.completeError(const DaemonStopped());
      }
      done.complete();
    }
  }

  /// One start: the daemon and its endpoint once it answers, or null when
  /// it exited, did not answer in time, could not be started, or [stop]
  /// was called.
  Future<(RunningProcess, DaemonEndpoint)?> _startOnce() async {
    RunningProcess? p;
    try {
      final exe = await _exe.clax();
      await _stopStale();
      if (_stopping) return null;
      for (final d in [_home, _support, _cache]) {
        Directory(d).createSync(recursive: true);
      }
      final port = await _ports.pick(preferred: _readPort());
      _writeSampleConfig();
      final environment = _environment();
      final key = (await _keyOrNull())?.trim();
      if (key != null && key.isNotEmpty) environment[keyEnv] = key;
      if (_stopping) return null;
      final proc = p = await _starter.start(
        exe,
        [
          '--port',
          '$port',
          'serve',
          '--foreground',
          if (_requireCredential) '--require-credential',
        ],
        environment: environment,
        workingDirectory: _home,
      );
      _proc = proc;
      _keepTail(proc);
      if (_stopping) {
        await _terminate(proc);
        return null;
      }
      int? code;
      final exited = proc.exitCode.then((c) => code = c);
      final deadline = _clock.now().add(healthLimit);
      while (true) {
        if (_stopping) return null;
        if (code != null) return _startExited(proc, code!);
        final info = _readDaemonJson();
        if (info != null && info.pid == proc.pid) {
          final base = Uri.parse('http://127.0.0.1:${info.port}');
          if (await _health.up(base)) {
            if (_stopping) return null;
            if (code != null) return _startExited(proc, code!);
            return (
              proc,
              DaemonEndpoint(
                base: base,
                token: info.token,
                pid: proc.pid,
                port: info.port,
              ),
            );
          }
        }
        if (!_clock.now().isBefore(deadline)) {
          _lastReason =
              'did not answer /healthz within ${healthLimit.inSeconds} s';
          proc.kill(ProcessSignal.sigkill);
          await proc.exitCode;
          _proc = null;
          _forgetDaemonJson(proc.pid);
          return null;
        }
        await Future.any([_clock.delay(pollInterval), exited]);
      }
    } on Object catch (e) {
      _lastReason = 'could not start clax: $e';
      _addTail('clax app: could not start clax: $e');
      final proc = p;
      if (proc != null) {
        proc.kill(ProcessSignal.sigkill);
        _proc = null;
      }
      return null;
    }
  }

  (RunningProcess, DaemonEndpoint)? _startExited(
    RunningProcess proc,
    int code,
  ) {
    _lastReason = 'exited with code $code while starting';
    _proc = null;
    _forgetDaemonJson(proc.pid);
    return null;
  }

  /// SIGTERM, then SIGKILL after [termGrace]; completes once [p] exited.
  Future<void> _terminate(RunningProcess p) async {
    var exited = false;
    final f = p.exitCode.then((_) => exited = true);
    p.kill(ProcessSignal.sigterm);
    await Future.any([f, _clock.delay(termGrace)]);
    if (!exited) {
      p.kill(ProcessSignal.sigkill);
      await f;
    }
  }

  /// Stops a live daemon that `daemon.json` names (one left by an earlier
  /// run of the app): SIGTERM, then SIGKILL after [termGrace].
  Future<void> _stopStale() async {
    final info = _readDaemonJson();
    final pid = info?.pid;
    if (pid == null || pid <= 0 || pid == io.pid) return;
    if (!_pids.alive(pid)) return;
    final port = info!.port;
    if (await _health.pid(Uri.parse('http://127.0.0.1:$port')) != pid) return;
    _pids.kill(pid, ProcessSignal.sigterm);
    if (await _waitGone(pid)) return;
    _pids.kill(pid, ProcessSignal.sigkill);
    await _waitGone(pid);
  }

  /// Whether [pid] is gone within [termGrace], checking every
  /// [pollInterval].
  Future<bool> _waitGone(int pid) async {
    final deadline = _clock.now().add(termGrace);
    while (_pids.alive(pid)) {
      if (!_clock.now().isBefore(deadline)) return false;
      await _clock.delay(pollInterval);
    }
    return true;
  }

  /// The saved key; null when there is none or it cannot be read (the
  /// daemon then starts with `sample()` off).
  Future<String?> _keyOrNull() async {
    try {
      return await _apiKey?.call();
    } catch (_) {
      return null;
    }
  }

  /// Writes [sampleConfig] to `<home>/config.toml` when it differs.
  void _writeSampleConfig() {
    final f = File('$_home/config.toml');
    try {
      if (f.existsSync() && f.readAsStringSync() == sampleConfig) return;
      f.writeAsStringSync(sampleConfig);
    } on FileSystemException {
      // The daemon starts with its own defaults.
    }
  }

  Map<String, String> _environment() => {
    'CLAX_HOME': _home,
    'HOME': _support,
    'TMPDIR': _cache,
    'PATH': _android ? '/system/bin' : '/usr/bin:/bin',
    'RUST_LOG': 'warn',
  };

  /// Keeps the last lines of both of [p]'s output streams; reading stdout
  /// also keeps a full pipe from blocking the daemon.
  void _keepTail(RunningProcess p) {
    for (final stream in [p.stdout, p.stderr]) {
      stream
          .transform(const Utf8Decoder(allowMalformed: true))
          .transform(const LineSplitter())
          .listen(_addTail, onError: (Object _) {});
    }
  }

  void _addTail(String line) {
    _tail.addLast(line);
    while (_tail.length > tailLines) {
      _tail.removeFirst();
    }
  }

  Future<void> _sleep(Duration d) async {
    final wake = _wake = Completer<void>();
    await Future.any([_clock.delay(d), wake.future]);
    _wake = null;
  }

  void _wakeUp() {
    final w = _wake;
    if (w != null && !w.isCompleted) w.complete();
  }

  void _set(DaemonState s) {
    _state = s;
    _states.add(s);
  }

  File get _portFile => File('$_support/daemon-port');
  File get _daemonJson => File('$_home/daemon.json');

  int? _readPort() {
    try {
      final p = int.tryParse(_portFile.readAsStringSync().trim());
      return p != null && p > 0 && p < 65536 ? p : null;
    } on FileSystemException {
      return null;
    }
  }

  /// Keeps [port] for the next start, written beside and renamed into
  /// place.
  void _writePort(int port) {
    try {
      final tmp = File('$_support/daemon-port.tmp')
        ..writeAsStringSync('$port\n', flush: true);
      tmp.renameSync(_portFile.path);
    } on FileSystemException {
      // Only the next start's port preference is lost.
    }
  }

  _DaemonInfo? _readDaemonJson() {
    try {
      final j = jsonDecode(_daemonJson.readAsStringSync());
      if (j is! Map<String, Object?>) return null;
      final pid = j['pid'], port = j['port'], token = j['token'];
      if (pid is! int || port is! int || token is! String) return null;
      return _DaemonInfo(pid, port, token);
    } on Object {
      return null;
    }
  }

  /// Removes `daemon.json` when it still names [pid], a daemon that has
  /// exited (a killed daemon leaves it behind), so a later start never
  /// signals a process that has since taken that PID.
  void _forgetDaemonJson(int pid) {
    if (_readDaemonJson()?.pid != pid) return;
    try {
      _daemonJson.deleteSync();
    } on FileSystemException {
      // Already gone.
    }
  }
}

class _DaemonInfo {
  const _DaemonInfo(this.pid, this.port, this.token);
  final int pid;
  final int port;
  final String token;
}
