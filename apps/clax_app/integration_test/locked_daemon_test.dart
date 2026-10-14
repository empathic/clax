// The app's WebView against a daemon started as the app starts it
// (`--require-credential`): the key cookie lets the shell load an app and
// fetch the token, which nothing without a credential gets.
//
// Run from apps/clax_app:
// `flutter test integration_test/locked_daemon_test.dart -d macos
//  --dart-define=CLAX_BIN=<absolute path of a debug clax>`.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/daemon/exe_locator.dart';
import 'package:clax_app/daemon/health.dart';
import 'package:clax_app/daemon/ports.dart';
import 'package:clax_app/daemon/process.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/daemon_cookie.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _bin = String.fromEnvironment('CLAX_BIN');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the shell loads an app and its token with the key cookie', (
    tester,
  ) async {
    final dir = await Directory.systemTemp.createTemp('clax-locked-');
    final home = Directory('${dir.path}/clax-home')..createSync();
    final supervisor = DaemonSupervisor(
      exe: _bin.isEmpty ? const MacExeLocator() : const FixedExeLocator(_bin),
      home: home.path,
      supportDir: dir.path,
      cacheDir: '${dir.path}/cache',
      starter: const IoProcessStarter(),
      ports: const LoopbackPortPicker(),
      health: HttpHealthProbe(),
      pids: IoPidKiller(),
      requireCredential: true,
    );
    addTearDown(() async {
      await supervisor.stop();
      await dir.delete(recursive: true);
    });
    final ep = await supervisor.ensureUp();
    final api = HttpClaxApi(() => ep);
    addTearDown(api.close);
    await api.registerAppSession(
      installId: 'locked-integration',
      pid: pid,
      cwd: dir.path,
    );
    final made = await api.createArtifact(
      PublishRequest(
        title: 'Locked',
        files: {'index.html': '<h1 id=t>Locked app</h1>'},
      ),
    );
    final aid = made.artifact.id;

    // Nothing without a credential.
    final client = HttpClient();
    addTearDown(() => client.close(force: true));
    final req = await client.getUrl(ep.base.replace(path: '/api/token'));
    req.headers.set('sec-fetch-site', 'same-origin');
    final res = await req.close();
    await res.drain<void>();
    expect(res.statusCode, 401);

    final surface = WebViewAppSurface(
      base: ep.base,
      keyCookie: DaemonKeyCookie(token: () => ep.token),
    );
    addTearDown(surface.dispose);
    final ready = Completer<Ready>();
    final sub = surface.messages.listen((m) {
      if (m is Ready && !ready.isCompleted) ready.complete(m);
    });
    addTearDown(sub.cancel);
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Builder(builder: surface.build),
      ),
    );
    await surface.load(AppScreen.appUrl(ep.base, aid));
    final r = await _until(tester, ready.future);
    expect(r.artifact, aid);

    // The shell's own token request answers it, as on a desktop.
    await surface.evaluate(
      "fetch('/api/token').then(r => { window.__tokenStatus = r.status; }); 0",
    );
    Object? status;
    for (var i = 0; i < 200 && status?.toString() != '200'; i++) {
      await tester.pump(const Duration(milliseconds: 50));
      status = await surface.evaluate('String(window.__tokenStatus)');
    }
    expect(status.toString().replaceAll('"', ''), '200');
  });
}

Future<T> _until<T>(WidgetTester tester, Future<T> f) async {
  T? value;
  var done = false;
  unawaited(
    f.then((v) {
      value = v;
      done = true;
    }),
  );
  for (var i = 0; i < 600 && !done; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
  expect(done, isTrue, reason: 'timed out waiting');
  return value as T;
}
