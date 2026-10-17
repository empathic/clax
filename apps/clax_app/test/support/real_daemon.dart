import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/daemon/supervisor.dart' show DaemonEndpoint;
import 'package:flutter_test/flutter_test.dart';
import 'paths.dart';

/// A real debug `clax` (the binary `CLAX_TEST_BIN` names) this test
/// started on a free port with a scratch home under the repository's
/// `target/flutter-app/`, stopped by its PID in a tear-down that runs on
/// a failure or a timeout too.
class RealDaemon {
  RealDaemon._(this.dir, this.process, this.endpoint);

  final Directory dir;
  final Process process;
  final DaemonEndpoint endpoint;

  static Future<RealDaemon> start(String bin) async {
    final scratch = scratchRoot();
    final dir = scratch.createTempSync('clax-real-');
    addTearDown(() {
      if (dir.existsSync()) dir.deleteSync(recursive: true);
    });
    final home = Directory('${dir.path}/clax-home')..createSync();
    final probe = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = probe.port;
    await probe.close();
    final p = await Process.start(
      bin,
      ['--port', '$port', 'serve', '--foreground'],
      environment: {
        'CLAX_HOME': home.path,
        'HOME': dir.path,
        'TMPDIR': dir.path,
        'PATH': '/usr/bin:/bin',
        'RUST_LOG': 'warn',
      },
      includeParentEnvironment: false,
      workingDirectory: dir.path,
    );
    // Registered before anything else can fail, and run on a timeout too.
    addTearDown(() => _stop(p));
    unawaited(p.stdout.drain<void>());
    final stderr = StringBuffer();
    p.stderr.transform(utf8.decoder).listen(stderr.write);
    var exited = false;
    unawaited(p.exitCode.then((_) => exited = true));
    // Up once daemon.json names this process and /healthz answers.
    final file = File('${home.path}/daemon.json');
    final client = HttpClient();
    final deadline = DateTime.now().add(const Duration(seconds: 90));
    try {
      while (true) {
        if (exited) throw StateError('clax exited before it was up: $stderr');
        if (DateTime.now().isAfter(deadline)) {
          p.kill(ProcessSignal.sigkill);
          throw StateError('clax was not up within 90 s: $stderr');
        }
        if (file.existsSync()) {
          try {
            final j =
                jsonDecode(file.readAsStringSync()) as Map<String, Object?>;
            if (j['pid'] == p.pid) {
              final req = await client.getUrl(
                Uri.parse('http://127.0.0.1:$port/healthz'),
              );
              final res = await req.close();
              await res.drain<void>();
              if (res.statusCode == 200) {
                return RealDaemon._(
                  dir,
                  p,
                  DaemonEndpoint(
                    base: Uri.parse('http://127.0.0.1:$port'),
                    token: j['token'] as String,
                    pid: p.pid,
                    port: port,
                  ),
                );
              }
            }
          } on FormatException catch (_) {
            // Written but not yet complete.
          } on IOException catch (_) {
            // Not listening yet, or the connection dropped mid-answer.
          }
        }
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    } on Object {
      p.kill(ProcessSignal.sigkill);
      rethrow;
    } finally {
      client.close(force: true);
    }
  }

  static Future<void> _stop(Process p) async {
    p.kill(ProcessSignal.sigterm);
    await p.exitCode.timeout(
      const Duration(seconds: 10),
      onTimeout: () {
        p.kill(ProcessSignal.sigkill);
        return p.exitCode;
      },
    );
  }
}
