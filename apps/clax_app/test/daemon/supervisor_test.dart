import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/daemon/exe_locator.dart';
import 'package:clax_app/daemon/health.dart';
import 'package:clax_app/daemon/ports.dart';
import 'package:clax_app/daemon/process.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_clock.dart';

class FakeProcess implements RunningProcess {
  FakeProcess(this.pid);
  @override
  final int pid;
  final _exit = Completer<int>();
  final _stderr = StreamController<List<int>>();
  final _stdout = StreamController<List<int>>();
  final List<ProcessSignal> signals = [];

  /// Whether the process ignores SIGTERM.
  bool ignoreTerm = false;

  @override
  Future<int> get exitCode => _exit.future;
  @override
  Stream<List<int>> get stderr => _stderr.stream;
  @override
  Stream<List<int>> get stdout => _stdout.stream;
  bool get exited => _exit.isCompleted;

  void exit(int code) {
    if (!_exit.isCompleted) _exit.complete(code);
  }

  void writeErr(String s) => _stderr.add(utf8.encode(s));
  void writeOut(String s) => _stdout.add(utf8.encode(s));

  @override
  bool kill([ProcessSignal s = ProcessSignal.sigterm]) {
    signals.add(s);
    if (exited) return false;
    if (s == ProcessSignal.sigterm && ignoreTerm) return true;
    exit(s == ProcessSignal.sigkill ? -9 : -15);
    return true;
  }
}

class StartCall {
  StartCall(
    this.exe,
    this.args,
    this.environment,
    this.workingDirectory,
    this.process,
  );
  final String exe;
  final List<String> args;
  final Map<String, String> environment;
  final String workingDirectory;
  final FakeProcess process;
  int get port => int.parse(args[1]);
}

/// Starts fake daemons. By default each one writes `daemon.json` (its PID,
/// the port it was given, a fresh token) at once, as the real daemon does
/// when it binds.
class FakeStarter implements ProcessStarter {
  FakeStarter(this.home);
  final String home;
  final List<StartCall> calls = [];
  int _nextPid = 5000;
  bool writeDaemonJson = true;

  /// Whether each started process exits at once, as one that cannot open
  /// its home does.
  bool exitAtOnce = false;

  @override
  Future<RunningProcess> start(
    String exe,
    List<String> args, {
    required Map<String, String> environment,
    required String workingDirectory,
  }) async {
    final p = FakeProcess(_nextPid++);
    calls.add(StartCall(exe, args, environment, workingDirectory, p));
    if (writeDaemonJson) {
      writeDaemon(
        home,
        pid: p.pid,
        port: int.parse(args[1]),
        token: 'tok-${calls.length}',
      );
    }
    if (exitAtOnce) p.exit(1);
    return p;
  }

  FakeProcess get last => calls.last.process;
}

void writeDaemon(
  String home, {
  required int pid,
  required int port,
  required String token,
}) {
  File('$home/daemon.json').writeAsStringSync(
    jsonEncode({
      'port': port,
      'pid': pid,
      'token': token,
      'started_at': '2026-10-12T09:00:00Z',
      'bind': '127.0.0.1',
      'version': '0.0.0',
    }),
  );
}

class FakePorts implements PortPicker {
  final List<int?> preferred = [];
  int free = 41000;
  @override
  Future<int> pick({int? preferred}) async {
    this.preferred.add(preferred);
    return preferred ?? free;
  }
}

class FakeHealth implements HealthProbe {
  bool answers = true;

  /// Answers `up` gives before falling back to [answers].
  final List<bool> next = [];

  /// The PID `/healthz` answers on each port.
  final Map<int, int> pidAt = {};
  final List<Uri> probed = [];
  @override
  Future<bool> up(Uri base) async {
    probed.add(base);
    return next.isNotEmpty ? next.removeAt(0) : answers;
  }

  @override
  Future<int?> pid(Uri base) async {
    probed.add(base);
    return pidAt[base.port];
  }
}

class FakePids implements PidKiller {
  final Set<int> live = {};
  final List<(int, ProcessSignal)> sent = [];
  bool ignoreTerm = false;
  @override
  bool alive(int pid) => live.contains(pid);
  @override
  void kill(int pid, ProcessSignal s) {
    sent.add((pid, s));
    if (s == ProcessSignal.sigterm && ignoreTerm) return;
    live.remove(pid);
  }
}

class Rig {
  Rig._(
    this.dir,
    this.starter,
    this.ports,
    this.health,
    this.pids,
    this.clock,
    this.sup,
  );

  factory Rig({
    DaemonHost host = DaemonHost.mac,
    Future<String?> Function()? apiKey,
    bool requireCredential = false,
  }) {
    final dir = Directory.systemTemp.createTempSync('clax-sup-');
    final home = '${dir.path}/support/clax-home';
    final starter = FakeStarter(home);
    final ports = FakePorts();
    final health = FakeHealth();
    final pids = FakePids();
    final clock = FakeClock();
    Directory(home).createSync(recursive: true);
    final sup = DaemonSupervisor(
      exe: const FixedExeLocator('/opt/clax'),
      home: home,
      supportDir: '${dir.path}/support',
      cacheDir: '${dir.path}/cache',
      starter: starter,
      ports: ports,
      health: health,
      pids: pids,
      clock: clock,
      host: host,
      apiKey: apiKey,
      requireCredential: requireCredential,
    );
    final rig = Rig._(dir, starter, ports, health, pids, clock, sup);
    sup.states.listen(rig.states.add);
    return rig;
  }

  final Directory dir;
  final FakeStarter starter;
  final FakePorts ports;
  final FakeHealth health;
  final FakePids pids;
  final FakeClock clock;
  final DaemonSupervisor sup;
  final List<DaemonState> states = [];

  String get home => '${dir.path}/support/clax-home';
  String get support => '${dir.path}/support';

  /// Calls `ensureUp` and runs the start to completion.
  Future<DaemonEndpoint> up() async {
    final f = sup.ensureUp();
    await clock.advance(Duration.zero);
    return f;
  }

  void dispose() => dir.deleteSync(recursive: true);
}

void main() {
  late Rig rig;
  tearDown(() => rig.dispose());

  test(
    'asks the daemon for a credential on every request when told to',
    () async {
      rig = Rig(requireCredential: true);
      final ep = await rig.up();
      expect(rig.starter.calls.single.args, [
        '--port',
        '41000',
        'serve',
        '--foreground',
        '--require-credential',
      ]);
      final c = keyCookie(ep.base, ep.token);
      expect(c.name, 'clax_key_${ep.base.port}');
      expect(c.value, ep.token);
      expect(c.domain, ep.base.host);
      expect(c.path, '/');
    },
  );

  test('starts with a clean environment and the expected arguments', () async {
    rig = Rig();
    final ep = await rig.up();
    expect(rig.starter.calls, hasLength(1));
    final call = rig.starter.calls.single;
    expect(call.exe, '/opt/clax');
    expect(call.args, ['--port', '41000', 'serve', '--foreground']);
    expect(call.environment.keys.toSet(), {
      'CLAX_HOME',
      'HOME',
      'TMPDIR',
      'PATH',
      'RUST_LOG',
    });
    expect(call.environment['CLAX_HOME'], rig.home);
    expect(call.environment['HOME'], rig.support);
    expect(call.environment['TMPDIR'], '${rig.dir.path}/cache');
    expect(call.environment['PATH'], '/usr/bin:/bin');
    expect(call.environment['RUST_LOG'], 'warn');
    expect(call.workingDirectory, rig.home);
    expect(ep.port, 41000);
    expect(ep.pid, call.process.pid);
    expect(ep.base, Uri.parse('http://127.0.0.1:41000'));
    expect(rig.states.first, isA<Starting>());
    expect(rig.states.last, isA<Up>());
    expect(rig.sup.state, isA<Up>());
  });

  test(
    'hands the saved API key to the daemon in its environment, never in a log',
    () async {
      const key = 'sk-ant-test-0123456789';
      var saved = key;
      final printed = <String>[];
      final old = debugPrint;
      debugPrint = (String? m, {int? wrapWidth}) => printed.add(m ?? '');
      addTearDown(() => debugPrint = old);
      await runZoned(
        () async {
          rig = Rig(apiKey: () async => saved);
          await rig.up();
          final env = rig.starter.calls.single.environment;
          expect(env[DaemonSupervisor.keyEnv], key);
          expect(
            File('${rig.home}/config.toml').readAsStringSync(),
            contains('default = "claude-haiku-5-5"'),
          );
          // The daemon fails five times, writing its output: the key appears
          // in no state, reason or message the supervisor gives.
          for (var i = 0; i < 5; i++) {
            rig.starter.last.writeErr('boom $i\n');
            rig.starter.last.exit(1);
            await rig.clock.advance(const Duration(seconds: 5));
          }
          expect(rig.sup.state, isA<Failed>());
          final failed = rig.sup.state as Failed;
          for (final s in rig.states) {
            expect(s.toString(), isNot(contains(key)));
          }
          expect(failed.reason, isNot(contains(key)));
          expect(failed.stderrTail.join('\n'), isNot(contains(key)));
          // No key: the daemon starts without the variable.
          saved = '';
          await rig.up();
        },
        zoneSpecification: ZoneSpecification(
          print: (self, parent, zone, line) => printed.add(line),
        ),
      );
      expect(
        rig.starter.calls.last.environment.containsKey(DaemonSupervisor.keyEnv),
        isFalse,
      );
      expect(printed.where((l) => l.contains(key)), isEmpty);
    },
  );

  test('restart starts the daemon again at once with the new key, counting no failure', () async {
    var saved = 'sk-old';
    rig = Rig(apiKey: () async => saved);
    await rig.up();
    saved = 'sk-new';
    rig.states.clear();
    final r = rig.sup.restart();
    await rig.clock.advance(Duration.zero);
    await r;
    expect(rig.starter.calls, hasLength(2));
    expect(rig.starter.calls.first.process.signals, [ProcessSignal.sigterm]);
    expect(
      rig.starter.calls.last.environment[DaemonSupervisor.keyEnv],
      'sk-new',
    );
    expect(rig.states.whereType<Restarting>(), isEmpty);
    expect(rig.sup.state, isA<Up>());
    // Four restarts more stay clear of the five-exits-a-minute failure.
    for (var i = 0; i < 4; i++) {
      final r = rig.sup.restart();
      await rig.clock.advance(Duration.zero);
      await r;
    }
    expect(rig.sup.state, isA<Up>());
    expect(rig.starter.calls, hasLength(6));
  });

  test('keyChanged restarts a child daemon to read the key from its environment', () async {
    var saved = 'sk-old';
    rig = Rig(apiKey: () async => saved);
    await rig.up();
    saved = 'sk-new';
    final r = rig.sup.keyChanged();
    await rig.clock.advance(Duration.zero);
    await r;
    expect(rig.starter.calls, hasLength(2));
    expect(rig.starter.calls.first.process.signals, [ProcessSignal.sigterm]);
    expect(
      rig.starter.calls.last.environment[DaemonSupervisor.keyEnv],
      'sk-new',
    );
  });

  test('uses the Android PATH on Android', () async {
    rig = Rig(host: DaemonHost.android);
    await rig.up();
    expect(rig.starter.calls.single.environment['PATH'], '/system/bin');
  });

  test('reuses the last port when it binds', () async {
    rig = Rig();
    File('${rig.support}/daemon-port').writeAsStringSync('43210\n');
    final ep = await rig.up();
    expect(rig.ports.preferred, [43210]);
    expect(ep.port, 43210);

    // The daemon may bind another port than the one asked for; the port it
    // reports in daemon.json is the endpoint's and the one kept.
    rig.dispose();
    rig = Rig();
    rig.starter.writeDaemonJson = false;
    final f = rig.sup.ensureUp();
    await rig.clock.advance(Duration.zero);
    expect(
      File('${rig.support}/daemon-port').existsSync(),
      isFalse,
      reason: 'the port file is written only once the daemon is up',
    );
    writeDaemon(rig.home, pid: rig.starter.last.pid, port: 41007, token: 't');
    await rig.clock.advance(const Duration(milliseconds: 50));
    final ep2 = await f;
    expect(ep2.port, 41007);
    expect(
      File('${rig.support}/daemon-port').readAsStringSync().trim(),
      '41007',
    );
  });

  test('a port file that is not a port is ignored', () async {
    rig = Rig();
    File('${rig.support}/daemon-port').writeAsStringSync('nope');
    await rig.up();
    expect(rig.ports.preferred, [null]);
  });

  test('stops a stale daemon from daemon.json first', () async {
    rig = Rig();
    writeDaemon(rig.home, pid: 777, port: 41000, token: 'old');
    rig.pids.live.add(777);
    rig.health.pidAt[41000] = 777;
    rig.pids.ignoreTerm = true;
    final f = rig.sup.ensureUp();
    await rig.clock.advance(Duration.zero);
    expect(rig.pids.sent, [(777, ProcessSignal.sigterm)]);
    expect(
      rig.starter.calls,
      isEmpty,
      reason: 'nothing starts while the stale daemon lives',
    );
    await rig.clock.advance(const Duration(milliseconds: 2950));
    expect(rig.pids.sent, hasLength(1));
    await rig.clock.advance(const Duration(milliseconds: 50));
    expect(rig.pids.sent, [
      (777, ProcessSignal.sigterm),
      (777, ProcessSignal.sigkill),
    ]);
    await rig.clock.advance(const Duration(milliseconds: 50));
    await f;
    expect(rig.starter.calls, hasLength(1));
  });

  test('a stale daemon that stops on SIGTERM is not killed', () async {
    rig = Rig();
    writeDaemon(rig.home, pid: 777, port: 41000, token: 'old');
    rig.pids.live.add(777);
    rig.health.pidAt[41000] = 777;
    await rig.up();
    expect(rig.pids.sent, [(777, ProcessSignal.sigterm)]);
    expect(rig.starter.calls, hasLength(1));
  });

  test('a live PID that /healthz does not answer as is left alone', () async {
    rig = Rig();
    writeDaemon(rig.home, pid: 777, port: 41000, token: 'old');
    rig.pids.live.add(777);
    // Another process took the PID; the port answers another daemon, or
    // nothing at all.
    rig.health.pidAt[41000] = 888;
    await rig.up();
    expect(rig.pids.sent, isEmpty);
    expect(rig.starter.calls, hasLength(1));

    rig.dispose();
    rig = Rig();
    writeDaemon(rig.home, pid: 777, port: 41000, token: 'old');
    rig.pids.live.add(777);
    await rig.up();
    expect(rig.pids.sent, isEmpty);
    expect(rig.starter.calls, hasLength(1));
  });

  test('a daemon.json naming a dead PID is left alone', () async {
    rig = Rig();
    writeDaemon(rig.home, pid: 777, port: 41000, token: 'old');
    await rig.up();
    expect(rig.pids.sent, isEmpty);
  });

  test('waits for health and reads the token from daemon.json', () async {
    rig = Rig();
    rig.health.answers = false;
    final f = rig.sup.ensureUp();
    var done = false;
    unawaited(f.then((_) => done = true));
    await rig.clock.advance(const Duration(milliseconds: 500));
    expect(done, isFalse);
    expect(rig.health.probed.length, greaterThanOrEqualTo(10));
    expect(rig.health.probed.toSet(), {Uri.parse('http://127.0.0.1:41000')});
    expect(rig.sup.state, isA<Starting>());
    rig.health.answers = true;
    await rig.clock.advance(const Duration(milliseconds: 50));
    final ep = await f;
    expect(ep.token, 'tok-1');
    expect(ep.pid, rig.starter.last.pid);
  });

  test('a daemon.json of another PID is not taken for this start', () async {
    rig = Rig();
    rig.starter.writeDaemonJson = false;
    writeDaemon(rig.home, pid: 999, port: 41000, token: 'someone-else');
    final f = rig.sup.ensureUp();
    await rig.clock.advance(const Duration(milliseconds: 200));
    expect(rig.health.probed, isEmpty);
    writeDaemon(
      rig.home,
      pid: rig.starter.last.pid,
      port: 41000,
      token: 'mine',
    );
    await rig.clock.advance(const Duration(milliseconds: 50));
    expect((await f).token, 'mine');
  });

  test('ensureUp is idempotent', () async {
    rig = Rig();
    final a = rig.sup.ensureUp();
    final b = rig.sup.ensureUp();
    await rig.clock.advance(Duration.zero);
    final ep = await a;
    expect(await b, same(ep));
    expect(await rig.sup.ensureUp(), same(ep));
    expect(rig.starter.calls, hasLength(1));
  });

  test('restarts an unexpected exit with backoff', () async {
    rig = Rig();
    final first = await rig.up();
    rig.states.clear();
    rig.starter.last.exit(1);
    await rig.clock.advance(Duration.zero);
    expect(rig.states.single, isA<Restarting>());
    final r = rig.states.single as Restarting;
    expect(r.attempt, 1);
    expect(r.after, const Duration(milliseconds: 200));
    expect(rig.starter.calls, hasLength(1));
    expect(
      File('${rig.home}/daemon.json').existsSync(),
      isFalse,
      reason: 'the dead daemon\'s daemon.json is removed',
    );
    await rig.clock.advance(const Duration(milliseconds: 199));
    expect(rig.starter.calls, hasLength(1));
    await rig.clock.advance(const Duration(milliseconds: 1));
    expect(rig.starter.calls, hasLength(2));
    expect(rig.states.last, isA<Up>());
    final second = (rig.states.last as Up).endpoint;
    expect(second.port, first.port, reason: 'the kept port is asked for again');
    expect(second.token, 'tok-2');
    expect(await rig.sup.ensureUp(), same(second));
  });

  test('backoff grows 0.2, 0.5, 1, 2 s and five exits within sixty seconds is Failed with the stderr tail', () async {
    rig = Rig();
    await rig.up();
    final afters = <Duration>[];
    for (var i = 0; i < 5; i++) {
      final p = rig.starter.last;
      // The daemon logs to stdout and fails on stderr; the tail keeps both.
      for (var l = 0; l < 15; l++) {
        p.writeOut('run $i line $l\n');
      }
      await settle();
      for (var l = 15; l < 30; l++) {
        p.writeErr('run $i line $l\n');
      }
      await settle();
      p.exit(101);
      await rig.clock.advance(Duration.zero);
      if (rig.sup.state is Restarting) {
        final r = rig.sup.state as Restarting;
        expect(r.attempt, i + 1);
        afters.add(r.after);
        await rig.clock.advance(r.after);
        expect(rig.sup.state, isA<Up>());
      }
    }
    expect(afters, const [
      Duration(milliseconds: 200),
      Duration(milliseconds: 500),
      Duration(seconds: 1),
      Duration(seconds: 2),
    ]);
    final failed = rig.sup.state as Failed;
    expect(failed.stderrTail, hasLength(50));
    expect(failed.stderrTail.last, 'run 4 line 29');
    expect(failed.stderrTail.first, 'run 3 line 10');
    expect(failed.stderrTail, contains('run 4 line 0'));
    expect(failed.reason, contains('101'));
    expect(rig.starter.calls, hasLength(5));
    await rig.clock.advance(const Duration(seconds: 30));
    expect(
      rig.starter.calls,
      hasLength(5),
      reason: 'nothing restarts once failed',
    );

    // Restart (ensureUp) starts afresh.
    final f = rig.sup.ensureUp();
    await rig.clock.advance(Duration.zero);
    await f;
    expect(rig.sup.state, isA<Up>());
    expect(rig.starter.calls, hasLength(6));
  });

  test('exits spread over more than sixty seconds never fail, and back off up to 5 s', () async {
    rig = Rig();
    await rig.up();
    final afters = <Duration>[];
    for (var i = 0; i < 8; i++) {
      rig.starter.last.exit(1);
      await rig.clock.advance(Duration.zero);
      final r = rig.sup.state as Restarting;
      afters.add(r.after);
      await rig.clock.advance(r.after);
      expect(rig.sup.state, isA<Up>());
      await rig.clock.advance(const Duration(seconds: 16));
    }
    expect(afters.take(5).toList(), const [
      Duration(milliseconds: 200),
      Duration(milliseconds: 500),
      Duration(seconds: 1),
      Duration(seconds: 2),
      Duration(seconds: 5),
    ]);
    expect(afters.every((a) => a <= const Duration(seconds: 5)), isTrue);
  });

  test('a waiter of ensureUp gets the failure', () async {
    rig = Rig();
    rig.starter.exitAtOnce = true;
    rig.health.answers = false;
    final f = rig.sup.ensureUp();
    Object? error;
    unawaited(f.then((_) {}, onError: (Object e) => error = e));
    await rig.clock.advance(const Duration(seconds: 10));
    expect(rig.sup.state, isA<Failed>());
    expect(error, isA<DaemonFailure>());
  });

  test('onResume restarts a dead daemon and leaves a live one', () async {
    rig = Rig();
    final ep = await rig.up();
    await rig.sup.onResume();
    await rig.clock.advance(Duration.zero);
    expect(rig.starter.calls, hasLength(1));
    expect(rig.sup.state, isA<Up>());
    expect(rig.health.probed.last, ep.base);

    // The daemon no longer answers (killed while the app was in the
    // background): it is restarted at once, without waiting out a backoff.
    rig.health.answers = false;
    final resumed = rig.sup.onResume();
    await rig.clock.advance(Duration.zero);
    await resumed;
    expect(rig.starter.calls.first.process.exited, isTrue);
    expect(rig.starter.calls, hasLength(2));
    rig.health.answers = true;
    await rig.clock.advance(const Duration(milliseconds: 50));
    expect(rig.sup.state, isA<Up>());
  });

  test(
    'onResume probes again before restarting a daemon slow to answer',
    () async {
      rig = Rig();
      await rig.up();
      rig.health.next.add(false);
      await rig.sup.onResume();
      await rig.clock.advance(Duration.zero);
      expect(rig.starter.calls, hasLength(1));
      expect(rig.starter.last.exited, isFalse);
      expect(rig.sup.state, isA<Up>());
    },
  );

  test('onResume starts afresh once Failed', () async {
    rig = Rig();
    await rig.up();
    for (var i = 0; i < 5; i++) {
      rig.starter.last.exit(1);
      await rig.clock.advance(Duration.zero);
      final s = rig.sup.state;
      if (s is Restarting) await rig.clock.advance(s.after);
    }
    expect(rig.sup.state, isA<Failed>());
    rig.states.clear();
    await rig.sup.onResume();
    await rig.clock.advance(Duration.zero);
    expect(rig.states.first, isA<Starting>());
    expect(rig.sup.state, isA<Up>());
    expect(rig.starter.calls, hasLength(6));
  });

  test('onResume after stop does nothing', () async {
    rig = Rig();
    await rig.up();
    await rig.sup.stop();
    await rig.sup.onResume();
    await rig.clock.advance(Duration.zero);
    expect(rig.starter.calls, hasLength(1));
  });

  test('onResume during a backoff restarts at once', () async {
    rig = Rig();
    await rig.up();
    rig.starter.last.exit(1);
    await rig.clock.advance(Duration.zero);
    expect(rig.sup.state, isA<Restarting>());
    await rig.sup.onResume();
    await rig.clock.advance(Duration.zero);
    expect(rig.starter.calls, hasLength(2));
    expect(rig.sup.state, isA<Up>());
  });

  test('onResume before any start does nothing', () async {
    rig = Rig();
    await rig.sup.onResume();
    await rig.clock.advance(Duration.zero);
    expect(rig.starter.calls, isEmpty);
    expect(rig.health.probed, isEmpty);
  });

  test('stop sends SIGTERM and does not restart', () async {
    rig = Rig();
    await rig.up();
    final p = rig.starter.last;
    rig.states.clear();
    await rig.sup.stop();
    expect(p.signals, [ProcessSignal.sigterm]);
    expect(p.exited, isTrue);
    await rig.clock.advance(const Duration(seconds: 10));
    expect(rig.starter.calls, hasLength(1));
    expect(rig.states.whereType<Restarting>(), isEmpty);

    // A later ensureUp starts it again.
    final f = rig.sup.ensureUp();
    await rig.clock.advance(Duration.zero);
    await f;
    expect(rig.starter.calls, hasLength(2));
  });

  test('stop kills a daemon that ignores SIGTERM after 3 s', () async {
    rig = Rig();
    await rig.up();
    final p = rig.starter.last..ignoreTerm = true;
    var stopped = false;
    unawaited(rig.sup.stop().then((_) => stopped = true));
    await rig.clock.advance(const Duration(milliseconds: 2999));
    expect(p.signals, [ProcessSignal.sigterm]);
    expect(stopped, isFalse);
    await rig.clock.advance(const Duration(milliseconds: 1));
    expect(p.signals, [ProcessSignal.sigterm, ProcessSignal.sigkill]);
    expect(stopped, isTrue);
  });

  test('stop during a start ends it without restarting', () async {
    rig = Rig();
    rig.health.answers = false;
    final f = rig.sup.ensureUp();
    Object? error;
    unawaited(f.then((_) {}, onError: (Object e) => error = e));
    await rig.clock.advance(const Duration(milliseconds: 100));
    final p = rig.starter.last;
    final s = rig.sup.stop();
    await rig.clock.advance(Duration.zero);
    await s;
    expect(p.exited, isTrue);
    expect(error, isA<DaemonStopped>());
    await rig.clock.advance(const Duration(seconds: 30));
    expect(rig.starter.calls, hasLength(1));
  });

  test('health that never answers within 15 s is a failed start', () async {
    rig = Rig();
    rig.health.answers = false;
    unawaited(rig.sup.ensureUp().then((_) {}, onError: (_) {}));
    await rig.clock.advance(const Duration(milliseconds: 14950));
    expect(rig.sup.state, isA<Starting>());
    final p = rig.starter.last;
    expect(p.exited, isFalse);
    await rig.clock.advance(const Duration(milliseconds: 50));
    expect(p.exited, isTrue, reason: 'the unanswering daemon is killed');
    expect(p.signals, contains(ProcessSignal.sigkill));
    final r = rig.sup.state as Restarting;
    expect(r.attempt, 1, reason: 'counts as one exit');
    expect(r.after, const Duration(milliseconds: 200));
  });

  test('an exit while starting is a failed start', () async {
    rig = Rig();
    rig.health.answers = false;
    unawaited(rig.sup.ensureUp().then((_) {}, onError: (_) {}));
    await rig.clock.advance(const Duration(milliseconds: 100));
    rig.starter.last
      ..writeErr('Error: opening the store\n')
      ..exit(1);
    await rig.clock.advance(Duration.zero);
    final r = rig.sup.state as Restarting;
    expect(r.attempt, 1);
  });

  test(
    'an exit for a failed database upgrade is Failed at once, with the error',
    () async {
      rig = Rig();
      rig.health.answers = false;
      Object? error;
      unawaited(
        rig.sup.ensureUp().then((_) {}, onError: (Object e) => error = e),
      );
      await rig.clock.advance(const Duration(milliseconds: 100));
      rig.starter.last
        ..writeErr(
          'error: could not upgrade the database from schema version 27: '
          'duplicate column name: draft_of\n',
        )
        ..exit(DaemonSupervisor.migrationFailedExit);
      await rig.clock.advance(Duration.zero);
      expect(rig.sup.state, isNot(isA<Restarting>()));
      // Its output never ends here: the tail is taken after the grace.
      await rig.clock.advance(DaemonSupervisor.termGrace);
      final failed = rig.sup.state as Failed;
      expect(failed.reason, contains('could not upgrade its database'));
      expect(failed.stderrTail.join('\n'), contains('schema version 27'));
      expect(error, isA<DaemonFailure>());
      await rig.clock.advance(const Duration(seconds: 30));
      expect(rig.starter.calls, hasLength(1), reason: 'not restarted');
    },
  );

  test('three starts in a row that never come up is Failed', () async {
    rig = Rig();
    rig.health.answers = false;
    Object? error;
    unawaited(
      rig.sup.ensureUp().then((_) {}, onError: (Object e) => error = e),
    );
    await rig.clock.advance(const Duration(seconds: 15));
    expect(rig.sup.state, isA<Restarting>());
    await rig.clock.advance(const Duration(milliseconds: 200));
    await rig.clock.advance(const Duration(seconds: 15));
    expect(rig.sup.state, isA<Restarting>());
    await rig.clock.advance(const Duration(milliseconds: 500));
    await rig.clock.advance(const Duration(seconds: 15));
    final failed = rig.sup.state as Failed;
    expect(failed.reason, contains('did not answer /healthz'));
    expect(rig.starter.calls, hasLength(3));
    expect(error, isA<DaemonFailure>());
  });

  test(
    'a start that comes up resets the count of starts that did not',
    () async {
      rig = Rig();
      rig.health.answers = false;
      unawaited(rig.sup.ensureUp().then((_) {}, onError: (_) {}));
      await rig.clock.advance(const Duration(milliseconds: 15200));
      await rig.clock.advance(const Duration(milliseconds: 15500));
      rig.health.answers = true;
      await rig.clock.advance(const Duration(milliseconds: 50));
      expect(rig.sup.state, isA<Up>());
      // Up for a minute, so the earlier exits leave the sixty-second window
      // and only the count of starts in a row could fail it.
      await rig.clock.advance(const Duration(seconds: 61));
      rig.health.answers = false;
      rig.starter.last.exit(1);
      await rig.clock.advance(const Duration(milliseconds: 200));
      await rig.clock.advance(const Duration(seconds: 15));
      await rig.clock.advance(const Duration(milliseconds: 500));
      await rig.clock.advance(const Duration(seconds: 15));
      expect(rig.sup.state, isA<Restarting>(), reason: 'two in a row so far');
      expect(rig.starter.calls, hasLength(5));
    },
  );

  test(
    'an executable that cannot be started fails after three tries',
    () async {
      final dir = Directory.systemTemp.createTempSync('clax-sup-');
      rig = Rig();
      final clock = FakeClock();
      final sup = DaemonSupervisor(
        exe: const FixedExeLocator('/opt/clax'),
        home: '${dir.path}/home',
        supportDir: '${dir.path}/support',
        cacheDir: '${dir.path}/cache',
        starter: _ThrowingStarter(),
        ports: FakePorts(),
        health: FakeHealth(),
        pids: FakePids(),
        clock: clock,
      );
      unawaited(sup.ensureUp().then((_) {}, onError: (_) {}));
      await clock.advance(const Duration(seconds: 10));
      final failed = sup.state as Failed;
      expect(failed.reason, contains('3 times in a row'));
      expect(failed.reason, contains('no such file'));
      dir.deleteSync(recursive: true);
    },
  );
}

class _ThrowingStarter implements ProcessStarter {
  @override
  Future<RunningProcess> start(
    String exe,
    List<String> args, {
    required Map<String, String> environment,
    required String workingDirectory,
  }) =>
      Future.error(const ProcessException('/opt/clax', [], 'no such file', 2));
}
