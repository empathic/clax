import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'paths.dart';
import 'settle.dart' show stuckAfter, waitUntil;

/// The `sh -c` script the daemon runs under
/// (`sh -c _held sh <clax> serve …`), as web/e2e/fixtures.ts runs its
/// daemons: a background reader of the shell's standard input (a pipe this process holds and never writes; fd
/// 3, since a background job's own input is /dev/null) sends the daemon
/// SIGTERM when that input ends, and the shell execs the daemon, which
/// keeps the shell's PID (so [Process.kill] and the exit code are the
/// daemon's). The input ends when the system closes this process's end of
/// it, as this process ends, however it ends.
const _held =
    r'exec 3<&0; (cat <&3; kill -TERM $$) >/dev/null 2>&1 & exec "$@" </dev/null 3<&-';

/// A hosted Clax (a public Clax): the `clax` binary CLAX_TEST_BIN names,
/// served in hosted mode on a scratch home under [scratchRoot] and a free
/// port of 127.0.0.1, its owner set up as "Ada" by the setup secret, and
/// stopped by its PID, then its home deleted, in tear-downs that run on a
/// failure or a timeout too. It runs under [_held], so a test process that
/// ends without its tear-downs leaves no daemon behind.
class HostedDaemon {
  HostedDaemon._(this.origin, this._cookie);

  /// Why a test of a hosted daemon cannot run here, or null when it can.
  static Object get skip =>
      claxBin() == null ? 'CLAX_TEST_BIN is not set' : false;

  static const _secret = 'the setup secret';

  /// `http://localhost:<port>`, the daemon's public origin.
  final String origin;

  /// The owner's browser session.
  final String _cookie;

  Uri get _base => Uri.parse(origin);

  /// Starts the daemon, waits until it answers, and sets its owner up.
  static Future<HostedDaemon> start() async {
    final home = scratchRoot().createTempSync('hosted-');
    addTearDown(() {
      if (home.existsSync()) home.deleteSync(recursive: true);
    });
    final free = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = free.port;
    await free.close();
    final origin = 'http://localhost:$port';
    File('${home.path}/config.toml').writeAsStringSync(
      '[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n'
      '[hosted]\npublic_origin = "$origin"\n'
      'setup_secret_env = "CLAX_E2E_SETUP_SECRET"\n',
    );
    final p = await Process.start(
      '/bin/sh',
      [
        '-c',
        _held,
        'sh',
        claxBin()!,
        'serve',
        '--foreground',
        '--bind',
        '127.0.0.1',
        '--port',
        '$port',
      ],
      environment: {
        'CLAX_HOME': home.path,
        'CLAX_CODEX_BIN': '',
        'CLAX_GALLERY__QUICKSTART': 'false',
        'CLAX_E2E_UNSET_KEY': '',
        'CLAX_E2E_SETUP_SECRET': _secret,
      },
    );
    final stderr = StringBuffer();
    p.stdout.drain<void>();
    p.stderr.transform(utf8.decoder).listen((s) {
      stderr.write(s);
    });
    var exited = false;
    unawaited(p.exitCode.then((_) => exited = true));
    addTearDown(() async {
      // SIGTERM to its PID; SIGKILL if its drain does not end.
      p.kill();
      final ended = await p.exitCode.timeout(stuckAfter, onTimeout: () => -1);
      if (ended == -1) {
        p.kill(ProcessSignal.sigkill);
        await p.exitCode;
      }
    });
    // Up when its daemon.json names its token and /healthz answers.
    final info = File('${home.path}/daemon.json');
    try {
      await waitUntil(() async {
        if (exited) {
          throw StateError(
            'the hosted daemon exited before it started: $stderr',
          );
        }
        try {
          final json = jsonDecode(info.readAsStringSync()) as Map;
          if (json['token'] is! String) return false;
          final (s, _, _) = await _send('GET', Uri.parse('$origin/healthz'));
          if (s == 200) return true;
        } on Object {
          // Not written, not whole, or not listening yet.
        }
        await Future<void>.delayed(const Duration(milliseconds: 20));
        return false;
      }, what: 'the hosted daemon to start');
    } on TestFailure {
      throw StateError('the hosted daemon did not start: $stderr');
    }
    final (s, body, headers) = await _send(
      'POST',
      Uri.parse('$origin/api/hosted/setup'),
      headers: {'origin': origin},
      body: {'secret': _secret, 'name': 'Ada'},
    );
    if (s != 200) throw StateError('setup answered $s: $body');
    final cookie = RegExp(
      r'clax_session=([^;]+)',
    ).firstMatch(headers['set-cookie'] ?? '')!.group(1)!;
    return HostedDaemon._(origin, cookie);
  }

  /// The owner approves device sign-in [userCode] on the approval page.
  Future<void> approve(String userCode) async {
    final (s, body, _) = await _send(
      'POST',
      _base.resolve('/api/hosted/device/${Uri.encodeComponent(userCode)}/approve'),
      headers: {'origin': origin, 'cookie': 'clax_session=$_cookie'},
      body: const {},
    );
    if (s != 200) throw StateError('the approval answered $s: $body');
  }

  /// A request to [url] on 127.0.0.1 with its own Host: its status, body
  /// and headers.
  static Future<(int, String, Map<String, String>)> _send(
    String method,
    Uri url, {
    Map<String, String> headers = const {},
    Object? body,
  }) async {
    final c = HttpClient();
    try {
      final r = await c.openUrl(method, url.replace(host: '127.0.0.1'));
      r.headers.set('host', '${url.host}:${url.port}');
      headers.forEach(r.headers.set);
      if (body != null) {
        r.headers.contentType = ContentType.json;
        r.write(jsonEncode(body));
      }
      final res = await r.close().timeout(stuckAfter);
      final text = await res.transform(utf8.decoder).join();
      final h = <String, String>{};
      res.headers.forEach((k, v) => h[k] = v.join(', '));
      return (res.statusCode, text, h);
    } finally {
      c.close(force: true);
    }
  }
}
