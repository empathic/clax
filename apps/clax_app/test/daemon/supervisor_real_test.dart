import 'dart:io';

import 'package:clax_app/daemon/exe_locator.dart';
import 'package:clax_app/daemon/health.dart';
import 'package:clax_app/daemon/ports.dart';
import 'package:clax_app/daemon/process.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:flutter_test/flutter_test.dart';
import '../support/paths.dart';

/// The supervisor against the real debug `clax` named by `CLAX_TEST_BIN`
/// (`tool/test-bin.sh` prints one), with a scratch home.
void main() {
  final bin = claxTestBin();

  test(
    'starts the real daemon, restarts it after SIGKILL on the same port, and stops it',
    () async {
      final dir = Directory.systemTemp.createTempSync('clax-sup-real-');
      addTearDown(() => dir.deleteSync(recursive: true));
      final pids = IoPidKiller();
      final sup = _supervisor(bin!, dir, pids);
      // Tear-downs run in reverse order, and also after a timeout: the
      // daemon is stopped before its home is deleted.
      addTearDown(sup.stop);
      final states = <DaemonState>[];
      final sub = sup.states.listen(states.add);
      addTearDown(sub.cancel);

      final ep = await sup.ensureUp();
      expect(await _status(ep, '/api/artifacts'), 200);

      final restarted = sup.states
          .where((s) => s is Up)
          .first
          .then((s) => (s as Up).endpoint);
      Process.killPid(ep.pid, ProcessSignal.sigkill);
      final again = await restarted;
      expect(states.whereType<Restarting>(), isNotEmpty);
      expect(again.pid, isNot(ep.pid));
      expect(again.port, ep.port);
      expect(again.token, isNot(ep.token));
      expect(await _status(again, '/api/artifacts'), 200);

      await sup.stop();
      expect(pids.alive(again.pid), isFalse);
    },
    skip: bin == null ? 'CLAX_TEST_BIN is not set' : false,
    // macOS assesses a new executable on its first run, which can take a while.
    timeout: const Timeout(Duration(minutes: 3)),
  );

  test(
    'a live process that daemon.json names but that is not the daemon is left alone',
    () async {
      final dir = Directory.systemTemp.createTempSync('clax-sup-real-');
      addTearDown(() => dir.deleteSync(recursive: true));
      final other = await Process.start('/bin/sleep', ['600']);
      addTearDown(() => other.kill(ProcessSignal.sigkill));
      final unused = await const LoopbackPortPicker().pick();
      final home = Directory('${dir.path}/support/clax-home')
        ..createSync(recursive: true);
      File('${home.path}/daemon.json').writeAsStringSync(
        '{"port": $unused, "pid": ${other.pid}, "token": "t", '
        '"started_at": "2026-10-12T09:00:00Z", "bind": "127.0.0.1", '
        '"version": "0"}',
      );
      final pids = IoPidKiller();
      final sup = _supervisor(bin!, dir, pids);
      addTearDown(sup.stop);

      final ep = await sup.ensureUp();
      expect(ep.pid, isNot(other.pid));
      expect(pids.alive(other.pid), isTrue);
    },
    skip: bin == null ? 'CLAX_TEST_BIN is not set' : false,
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

DaemonSupervisor _supervisor(String bin, Directory dir, PidKiller pids) =>
    DaemonSupervisor(
      exe: FixedExeLocator(bin),
      home: '${dir.path}/support/clax-home',
      supportDir: '${dir.path}/support',
      cacheDir: '${dir.path}/cache',
      starter: const IoProcessStarter(),
      ports: const LoopbackPortPicker(),
      health: HttpHealthProbe(),
      pids: pids,
    );

Future<int> _status(DaemonEndpoint ep, String path) async {
  final client = HttpClient();
  try {
    final req = await client.getUrl(ep.base.resolve(path));
    req.headers.set(HttpHeaders.authorizationHeader, 'Bearer ${ep.token}');
    final res = await req.close();
    await res.drain<void>();
    return res.statusCode;
  } finally {
    client.close(force: true);
  }
}
