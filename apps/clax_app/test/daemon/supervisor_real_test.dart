import 'dart:io';

import 'package:clax_app/daemon/exe_locator.dart';
import 'package:clax_app/daemon/health.dart';
import 'package:clax_app/daemon/ports.dart';
import 'package:clax_app/daemon/process.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:flutter_test/flutter_test.dart';

/// The supervisor against the real debug `clax` named by `CLAX_TEST_BIN`
/// (`tool/test-bin.sh` prints one), with a scratch home.
void main() {
  final bin = Platform.environment['CLAX_TEST_BIN'];

  test(
    'starts the real daemon, restarts it after SIGKILL on the same port, and stops it',
    () async {
      final dir = Directory.systemTemp.createTempSync('clax-sup-real-');
      final pids = IoPidKiller();
      final sup = DaemonSupervisor(
        exe: FixedExeLocator(bin!),
        home: '${dir.path}/support/clax-home',
        supportDir: '${dir.path}/support',
        cacheDir: '${dir.path}/cache',
        starter: const IoProcessStarter(),
        ports: const LoopbackPortPicker(),
        health: HttpHealthProbe(),
        pids: pids,
      );
      final states = <DaemonState>[];
      final sub = sup.states.listen(states.add);
      try {
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
      } finally {
        await sub.cancel();
        await sup.stop();
        dir.deleteSync(recursive: true);
      }
    },
    skip: bin == null ? 'CLAX_TEST_BIN is not set' : false,
    // macOS assesses a new executable on its first run, which can take a while.
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

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
