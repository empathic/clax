// A manual check that the embedded clax runs inside the macOS App Sandbox.
//
// Build it as the app's entry point in Release (the only sandboxed
// configuration) and run the binary from a terminal:
//
//   flutter build macos --release -t tool/sandbox_probe.dart
//   build/macos/Build/Products/Release/Clax.app/Contents/MacOS/Clax
//
// It runs Contents/Resources/clax --version, then starts `clax serve` with
// a CLAX_HOME in the app's container on a free loopback port, waits up to
// 15 s for /healthz, stops the daemon by its PID and exits: 0 when both
// steps work, 1 otherwise. Each step prints one line, and the lines are
// also written to <application support>/sandbox-probe.txt.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:path_provider/path_provider.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final support = await getApplicationSupportDirectory();
  final lines = <String>[];
  void say(String line) {
    lines.add(line);
    stdout.writeln('sandbox-probe: $line');
  }

  var ok = false;
  try {
    ok = await _probe(support, say);
  } catch (e) {
    say('error: $e');
  }
  say(ok ? 'PASS' : 'FAIL');
  File('${support.path}/sandbox-probe.txt').writeAsStringSync('${lines.join('\n')}\n');
  await stdout.flush();
  exit(ok ? 0 : 1);
}

Future<bool> _probe(Directory support, void Function(String) say) async {
  final exe = File(Platform.resolvedExecutable);
  final clax = '${exe.parent.parent.path}/Resources/clax';
  say('clax: $clax');
  say('sandbox container: ${Platform.environment['APP_SANDBOX_CONTAINER_ID'] ?? '(none)'}');
  final home = Directory('${support.path}/probe-home')..createSync(recursive: true);
  final env = {
    'CLAX_HOME': home.path,
    'HOME': Platform.environment['HOME'] ?? support.path,
    'TMPDIR': Directory.systemTemp.path,
    'PATH': '/usr/bin:/bin',
  };

  final version = await Process.run(clax, ['--version'],
      environment: env, includeParentEnvironment: false);
  say('--version: exit ${version.exitCode}, ${'${version.stdout}'.trim()}');
  if (version.exitCode != 0) return false;

  final socket = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
  final port = socket.port;
  await socket.close();
  final daemon = await Process.start(
      clax, ['--port', '$port', 'serve', '--foreground'],
      environment: env, includeParentEnvironment: false);
  final stderrTail = <String>[];
  daemon.stderr.transform(utf8.decoder).transform(const LineSplitter()).listen((l) {
    stderrTail.add(l);
    if (stderrTail.length > 10) stderrTail.removeAt(0);
  });
  daemon.stdout.drain<void>();
  var exited = false;
  unawaited(daemon.exitCode.then((_) => exited = true));
  say('serve: pid ${daemon.pid}, port $port');

  final client = HttpClient()..connectionTimeout = const Duration(seconds: 1);
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  var up = false;
  while (!up && !exited && DateTime.now().isBefore(deadline)) {
    try {
      final req = await client.getUrl(Uri.parse('http://127.0.0.1:$port/healthz'));
      final res = await req.close();
      await res.drain<void>();
      up = res.statusCode == 200;
    } catch (_) {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
  }
  client.close(force: true);
  say(up ? 'healthz: 200' : 'healthz: no answer (exited: $exited) ${stderrTail.join(' | ')}');
  daemon.kill(ProcessSignal.sigterm);
  final code = await daemon.exitCode.timeout(const Duration(seconds: 5), onTimeout: () {
    daemon.kill(ProcessSignal.sigkill);
    return -9;
  });
  say('serve exit: $code');
  return up;
}
