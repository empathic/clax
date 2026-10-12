import 'dart:async';
import 'dart:io';

/// A started child process.
abstract interface class RunningProcess {
  int get pid;

  /// Completes with the exit code once the process has exited.
  Future<int> get exitCode;

  /// The child's standard error.
  Stream<List<int>> get stderr;

  /// The child's standard output. It must be read, or a full pipe blocks
  /// the child.
  Stream<List<int>> get stdout;

  /// Sends [s]; false when it could not be sent.
  bool kill([ProcessSignal s = ProcessSignal.sigterm]);
}

/// Starts child processes.
abstract interface class ProcessStarter {
  /// Starts [exe] with [args], with exactly [environment] (nothing is
  /// inherited) in [workingDirectory].
  Future<RunningProcess> start(
    String exe,
    List<String> args, {
    required Map<String, String> environment,
    required String workingDirectory,
  });
}

/// Starts real processes with `Process.start`, without the parent's
/// environment. The child's stdin is closed at once.
class IoProcessStarter implements ProcessStarter {
  const IoProcessStarter();

  @override
  Future<RunningProcess> start(
    String exe,
    List<String> args, {
    required Map<String, String> environment,
    required String workingDirectory,
  }) async {
    final p = await Process.start(
      exe,
      args,
      environment: environment,
      workingDirectory: workingDirectory,
      includeParentEnvironment: false,
    );
    unawaited(p.stdin.close().catchError((Object _) {}));
    return _IoRunningProcess(p);
  }
}

class _IoRunningProcess implements RunningProcess {
  _IoRunningProcess(this._p);
  final Process _p;

  @override
  int get pid => _p.pid;
  @override
  Future<int> get exitCode => _p.exitCode;
  @override
  Stream<List<int>> get stderr => _p.stderr;
  @override
  Stream<List<int>> get stdout => _p.stdout;
  @override
  bool kill([ProcessSignal s = ProcessSignal.sigterm]) => _p.kill(s);
}

/// Checks and signals processes by PID (processes this app did not start
/// in this run, such as a daemon left by an earlier run).
abstract interface class PidKiller {
  /// Whether a process with [pid] exists and this app may signal it.
  bool alive(int pid);

  void kill(int pid, ProcessSignal s);
}

/// [PidKiller] over `Process.killPid`. Liveness is `/proc/<pid>` on Linux
/// and Android (where other apps' processes are hidden) and `kill -0`
/// elsewhere (which fails for processes of other users).
class IoPidKiller implements PidKiller {
  @override
  bool alive(int pid) {
    if (pid <= 0) return false;
    if (Platform.isLinux || Platform.isAndroid) {
      return Directory('/proc/$pid').existsSync();
    }
    try {
      return Process.runSync('/bin/kill', ['-0', '$pid']).exitCode == 0;
    } on ProcessException {
      return false;
    }
  }

  @override
  void kill(int pid, ProcessSignal s) => Process.killPid(pid, s);
}
