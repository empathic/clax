import 'dart:async';
import 'dart:io';
import 'dart:io' as io show pid;

import 'package:flutter/services.dart';

import 'clock.dart';
import 'exe_locator.dart';
import 'process.dart';

/// An [ExeLocator] for `DaemonHost.inProcess`: there is no executable.
class InProcessLocator implements ExeLocator {
  const InProcessLocator();

  @override
  Future<String> clax() async => 'in-process';
}

/// A [ProcessStarter] over the daemon inside this app (`clax_start` and
/// friends, served by `ios/Runner/NativeChannel.swift` on
/// `dev.empathic.clax/native`): `start` sends `daemonStart {port}` with
/// the port of `--port <P>` in its arguments (0 without one); the
/// [RunningProcess]'s `pid` is this process's, its `exitCode` completes
/// with `daemonState − 100` once a `daemonState` poll (every [pollEvery] on
/// [clock]) answers 100 or more, `kill` sends `daemonStop` (any signal),
/// and `stderr` carries the bytes appended to [logPath] since the start
/// (read on each poll, and from the start of a new file once the log is
/// cut); `stdout` is empty. `daemonStart` answering anything but 0 (1:
/// already running, 2: no `$HOME`) is a start that exited with that
/// status. The environment and working directory are not used: the daemon
/// takes its home from the app's container.
class InProcessStarter implements ProcessStarter {
  InProcessStarter({
    required this.logPath,
    this.channel = const MethodChannel('dev.empathic.clax/native'),
    this.clock = const SystemClock(),
    this.pollEvery = const Duration(milliseconds: 250),
  });

  final String logPath;
  final MethodChannel channel;
  final Clock clock;
  final Duration pollEvery;

  @override
  Future<RunningProcess> start(
    String exe,
    List<String> args, {
    required Map<String, String> environment,
    required String workingDirectory,
  }) async {
    final at = args.indexOf('--port');
    final port = at >= 0 && at + 1 < args.length
        ? int.tryParse(args[at + 1]) ?? 0
        : 0;
    final daemon = _InProcessDaemon(this, _length(logPath));
    final answer = await channel.invokeMethod<int>('daemonStart', {
      'port': port,
    });
    if (answer == 0) {
      unawaited(daemon._poll());
    } else {
      daemon._end(answer ?? 1);
    }
    return daemon;
  }
}

int _length(String path) {
  try {
    return File(path).lengthSync();
  } on FileSystemException {
    return 0;
  }
}

class _InProcessDaemon implements RunningProcess {
  _InProcessDaemon(this._starter, this._offset);

  final InProcessStarter _starter;
  int _offset;
  final _exit = Completer<int>();
  final _stderr = StreamController<List<int>>();

  @override
  int get pid => io.pid;

  @override
  Future<int> get exitCode => _exit.future;

  @override
  Stream<List<int>> get stderr => _stderr.stream;

  @override
  Stream<List<int>> get stdout => const Stream.empty();

  @override
  bool kill([ProcessSignal s = ProcessSignal.sigterm]) {
    if (_exit.isCompleted) return false;
    unawaited(
      _starter.channel
          .invokeMethod<void>('daemonStop')
          .then((_) {}, onError: (Object _) {}),
    );
    return true;
  }

  Future<void> _poll() async {
    while (!_exit.isCompleted) {
      await _starter.clock.delay(_starter.pollEvery);
      int? state;
      try {
        state = await _starter.channel.invokeMethod<int>('daemonState');
      } on Object {
        state = null;
      }
      if (state != null && state >= 100) {
        _end(state - 100);
      } else {
        _readLog();
      }
    }
  }

  void _end(int code) {
    if (_exit.isCompleted) return;
    _readLog();
    unawaited(_stderr.close());
    _exit.complete(code);
  }

  /// Adds what was appended to the log since the last read.
  void _readLog() {
    final f = File(_starter.logPath);
    try {
      final length = f.lengthSync();
      // The log was cut (`daemon.log.1`): the new file is read from its
      // start.
      if (length < _offset) _offset = 0;
      if (length == _offset) return;
      final raf = f.openSync();
      try {
        raf.setPositionSync(_offset);
        final bytes = raf.readSync(length - _offset);
        _offset += bytes.length;
        if (bytes.isNotEmpty) _stderr.add(bytes);
      } finally {
        raf.closeSync();
      }
    } on FileSystemException {
      // No log yet.
    }
  }
}
