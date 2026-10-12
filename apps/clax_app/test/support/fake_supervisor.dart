import 'dart:async';

import 'package:clax_app/daemon/supervisor.dart';

/// A [DaemonSupervisor] for tests: the test sets its state ([set]), and
/// [ensureUp] waits for the next `Up` (or throws on `Failed`), as the real
/// one does. Every call is counted.
class FakeSupervisor implements DaemonSupervisor {
  final _states = StreamController<DaemonState>.broadcast(sync: true);
  DaemonState _state = const Starting();
  Completer<DaemonEndpoint>? _waiter;

  int ensureUpCalls = 0;
  int onResumeCalls = 0;
  int stopCalls = 0;

  @override
  Stream<DaemonState> get states => _states.stream;

  @override
  DaemonState get state => _state;

  /// Moves to [s] and tells listeners; an `Up` or `Failed` answers a
  /// waiting [ensureUp].
  void set(DaemonState s) {
    _state = s;
    _states.add(s);
    final w = _waiter;
    if (s is Up) {
      _waiter = null;
      w?.complete(s.endpoint);
    } else if (s is Failed) {
      _waiter = null;
      w?.completeError(DaemonFailure(s.reason, s.stderrTail));
    }
  }

  @override
  Future<DaemonEndpoint> ensureUp() {
    ensureUpCalls++;
    final s = _state;
    if (s is Up) return Future.value(s.endpoint);
    return (_waiter ??= Completer<DaemonEndpoint>()).future;
  }

  @override
  Future<void> onResume() async => onResumeCalls++;

  @override
  Future<void> stop() async => stopCalls++;
}

/// An endpoint on [port] for daemon [pid].
DaemonEndpoint endpoint({int port = 7999, int pid = 100}) => DaemonEndpoint(
  base: Uri.parse('http://127.0.0.1:$port'),
  token: 'token-$pid',
  pid: pid,
  port: port,
);
