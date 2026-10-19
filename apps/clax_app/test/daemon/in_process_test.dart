import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:io' as io show pid;

import 'package:clax_app/daemon/health.dart';
import 'package:clax_app/daemon/in_process.dart';
import 'package:clax_app/daemon/ports.dart';
import 'package:clax_app/daemon/process.dart';
import 'package:clax_app/daemon/sample_key.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_clock.dart';

const _channel = MethodChannel('dev.empathic.clax/native');

/// The daemon inside the app, as `ios/Runner/NativeChannel.swift` serves it
/// on [_channel]: `daemonStart {port}` writes `daemon.json` naming this
/// process and answers 0 (1 while one is starting, up or stopping);
/// `daemonStop` moves to 3, and the `daemonState` poll after the one that
/// answers 3 answers 100. With [failMigration] a start writes two lines to
/// the log and answers 165 from then on.
class FakeNativeDaemon {
  FakeNativeDaemon({required this.home, required this.log});

  final String home;
  final String log;
  final List<MethodCall> calls = [];

  /// Every `daemonState` answer, in order.
  final List<int> answered = [];
  int state = 0;
  bool failMigration = false;
  int _tokens = 0;

  int count(String method) => calls.where((c) => c.method == method).length;

  List<int> get startPorts => [
    for (final c in calls)
      if (c.method == 'daemonStart') (c.arguments as Map)['port'] as int,
  ];

  Future<Object?> handle(MethodCall call) async {
    calls.add(call);
    switch (call.method) {
      case 'daemonStart':
        if (state >= 1 && state <= 3) return 1;
        final port = (call.arguments as Map)['port'] as int;
        if (failMigration) {
          File(log).writeAsStringSync(
            'ERROR clax: could not upgrade the database from schema version 27\n'
            'ERROR clax: duplicate column name: draft_of\n',
            mode: FileMode.append,
          );
          state = 165;
          return 0;
        }
        state = 2;
        File('$home/daemon.json').writeAsStringSync(
          jsonEncode({
            'port': port,
            'pid': io.pid,
            'token': 'tok-${++_tokens}',
          }),
        );
        return 0;
      case 'daemonStop':
        if (state >= 1 && state <= 2) state = 3;
        return null;
      case 'daemonState':
        final s = state;
        answered.add(s);
        if (s == 3) {
          state = 100;
          final f = File('$home/daemon.json');
          if (f.existsSync()) f.deleteSync();
        }
        return s;
    }
    throw MissingPluginException(call.method);
  }
}

class FakeSampleKeyPort implements SampleKeyPort {
  final List<(String, String, String?)> puts = [];

  @override
  Future<bool> put(Uri base, String token, String? key) async {
    puts.add(('$base', token, key));
    return true;
  }
}

/// Fails the test on any call: an in-process daemon is never signalled.
class ThrowingPids implements PidKiller {
  final List<int> calls = [];
  @override
  bool alive(int pid) {
    calls.add(pid);
    throw StateError('alive($pid) called for an in-process daemon');
  }

  @override
  void kill(int pid, ProcessSignal s) {
    calls.add(pid);
    throw StateError('kill($pid) called for an in-process daemon');
  }
}

/// The preferred port when there is one, else 41000, 41001, …
class FakePorts implements PortPicker {
  final List<int?> preferred = [];
  int free = 41000;
  @override
  Future<int> pick({int? preferred}) async {
    this.preferred.add(preferred);
    return preferred ?? free++;
  }
}

class FakeHealth implements HealthProbe {
  bool answers = true;
  final List<bool> next = [];
  @override
  Future<bool> up(Uri base) async =>
      next.isNotEmpty ? next.removeAt(0) : answers;
  @override
  Future<int?> pid(Uri base) async => io.pid;
}

/// Records each start's environment and hands it to [inner].
class RecordingStarter implements ProcessStarter {
  RecordingStarter(this.inner);
  final ProcessStarter inner;
  final List<Map<String, String>> environments = [];

  @override
  Future<RunningProcess> start(
    String exe,
    List<String> args, {
    required Map<String, String> environment,
    required String workingDirectory,
  }) {
    environments.add(environment);
    return inner.start(
      exe,
      args,
      environment: environment,
      workingDirectory: workingDirectory,
    );
  }
}

class Rig {
  Rig() {
    Directory(home).createSync(recursive: true);
    Directory(cache).createSync(recursive: true);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, daemon.handle);
    sup = DaemonSupervisor(
      exe: const InProcessLocator(),
      home: home,
      supportDir: support,
      cacheDir: cache,
      starter: starter,
      ports: ports,
      health: health,
      pids: pids,
      clock: clock,
      host: DaemonHost.inProcess,
      keyPort: keys,
      apiKey: () async => saved,
      requireCredential: true,
    );
  }

  final dir = Directory.systemTemp.createTempSync('clax-inproc-');
  String get support => '${dir.path}/support';
  String get home => '$support/clax-home';
  String get cache => '${dir.path}/cache';
  String get log => '$cache/daemon.log';

  final clock = FakeClock();
  late final daemon = FakeNativeDaemon(home: home, log: log);
  late final starter = RecordingStarter(
    InProcessStarter(logPath: log, clock: clock),
  );
  final ports = FakePorts();
  final health = FakeHealth();
  final pids = ThrowingPids();
  final keys = FakeSampleKeyPort();
  String? saved = 'sk-1';
  late final DaemonSupervisor sup;

  Future<DaemonEndpoint> up() async {
    final f = sup.ensureUp();
    await clock.advance(Duration.zero);
    return f;
  }

  void dispose() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, null);
    dir.deleteSync(recursive: true);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late Rig rig;
  setUp(() => rig = Rig());
  tearDown(() => rig.dispose());

  test(
    'a start asks for the picked port, is up on daemon.json and /healthz, '
    'and puts the saved key, which no environment carries',
    () async {
      // An earlier run of the app (killed while suspended) left its PID.
      File('${rig.home}/daemon.json').writeAsStringSync(
        jsonEncode({'port': 40000, 'pid': 4242, 'token': 'old'}),
      );
      final ep = await rig.up();
      expect(rig.daemon.startPorts, [41000]);
      expect(rig.sup.state, isA<Up>());
      expect(ep.pid, io.pid);
      expect(ep.port, 41000);
      expect(ep.token, 'tok-1');
      expect(rig.keys.puts, [('http://127.0.0.1:41000', 'tok-1', 'sk-1')]);
      expect(rig.starter.environments.single, isEmpty);
      expect(rig.pids.calls, isEmpty);
    },
  );

  test('keyChanged while up puts the new key and starts nothing', () async {
    await rig.up();
    rig.saved = 'sk-2';
    await rig.sup.keyChanged();
    await rig.clock.advance(Duration.zero);
    rig.saved = null;
    await rig.sup.keyChanged();
    await rig.clock.advance(Duration.zero);
    expect(rig.keys.puts, [
      ('http://127.0.0.1:41000', 'tok-1', 'sk-1'),
      ('http://127.0.0.1:41000', 'tok-1', 'sk-2'),
      ('http://127.0.0.1:41000', 'tok-1', null),
    ]);
    expect(rig.daemon.count('daemonStart'), 1);
    expect(rig.daemon.count('daemonStop'), 0);
    expect(rig.sup.state, isA<Up>());
  });

  test('a key changed while starting is put once the daemon is up', () async {
    rig.health.answers = false;
    final f = rig.sup.ensureUp();
    await rig.clock.advance(const Duration(milliseconds: 100));
    expect(rig.sup.state, isA<Starting>());
    rig.saved = 'sk-2';
    await rig.sup.keyChanged();
    await rig.clock.advance(Duration.zero);
    expect(rig.keys.puts, isEmpty);
    rig.health.answers = true;
    await rig.clock.advance(const Duration(milliseconds: 100));
    await f;
    expect(rig.keys.puts, [('http://127.0.0.1:41000', 'tok-1', 'sk-2')]);
    expect(rig.daemon.count('daemonStart'), 1);
  });

  test(
    'resume with a dead listener restarts on the same port and puts the key again',
    () async {
      await rig.up();
      rig.health.next.addAll([false, false]);
      final resumed = rig.sup.onResume();
      await rig.clock.advance(Duration.zero);
      await resumed;
      expect(rig.daemon.count('daemonStop'), 1);
      await rig.clock.advance(const Duration(seconds: 1));
      expect(rig.daemon.answered, containsAllInOrder([3, 100]));
      expect(rig.daemon.startPorts, [41000, 41000]);
      expect(rig.ports.preferred.last, 41000);
      final up = rig.sup.state as Up;
      expect(up.endpoint.port, 41000);
      expect(up.endpoint.token, 'tok-2');
      expect(rig.keys.puts, [
        ('http://127.0.0.1:41000', 'tok-1', 'sk-1'),
        ('http://127.0.0.1:41000', 'tok-2', 'sk-1'),
      ]);
      expect(rig.pids.calls, isEmpty);
    },
  );

  test(
    'a failed migration is Failed at once, with the log lines since the start',
    () async {
      File(rig.log).writeAsStringSync('INFO clax: an earlier run\n');
      rig.daemon.failMigration = true;
      Object? error;
      unawaited(
        rig.sup.ensureUp().then((_) {}, onError: (Object e) => error = e),
      );
      await rig.clock.advance(const Duration(seconds: 1));
      final failed = rig.sup.state as Failed;
      expect(failed.reason, contains('could not upgrade its database'));
      expect(failed.stderrTail, [
        'ERROR clax: could not upgrade the database from schema version 27',
        'ERROR clax: duplicate column name: draft_of',
      ]);
      expect(error, isA<DaemonFailure>());
      await rig.clock.advance(const Duration(seconds: 30));
      expect(rig.daemon.count('daemonStart'), 1, reason: 'not restarted');
    },
  );

  test('stop sends daemonStop and completes once the state reaches 100', () async {
    await rig.up();
    var stopped = false;
    final s = rig.sup.stop().then((_) => stopped = true);
    await rig.clock.advance(Duration.zero);
    expect(rig.daemon.count('daemonStop'), 1);
    expect(stopped, isFalse);
    await rig.clock.advance(const Duration(seconds: 1));
    await s;
    expect(rig.daemon.state, 100);
    expect(rig.daemon.count('daemonStop'), 1);
    await rig.clock.advance(const Duration(seconds: 10));
    expect(rig.daemon.count('daemonStart'), 1);
    expect(rig.pids.calls, isEmpty);
  });

  test('a start the daemon refuses as already running exits with 1', () async {
    rig.daemon.state = 2;
    final p = await InProcessStarter(logPath: rig.log, clock: rig.clock).start(
      'in-process',
      ['--port', '41000', 'serve', '--foreground'],
      environment: const {},
      workingDirectory: rig.home,
    );
    expect(p.pid, io.pid);
    expect(await p.exitCode, 1);
    expect(await p.stdout.isEmpty, isTrue);
  });
}
