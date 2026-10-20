import 'dart:async';
import 'dart:convert';

import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/core/scheme.dart';
import 'package:clax_app/core/scheme_host.dart';
import 'package:clax_app/core/shell_link.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

import '../support/fake_core.dart';

void main() {
  final base = Uri.parse('http://127.0.0.1:7999');
  late List<Uri> opened;
  setUp(() => opened = []);

  NavigationDecision top(String url) => WebViewAppSurface.decide(
    base,
    url,
    isMainFrame: true,
    openExternal: opened.add,
  );

  test('the top level may load the shell app view', () {
    for (final path in [
      '/a/abcdefghjkmn?app=1',
      '/a/abcdefghjkmn/?app=1',
      '/a/abcdefghjkmn/v/3?app=1#top',
      '/a/abcdefghjkmn/about.html?app=1',
      '/a/abcdefghjkmn/v/3/pages/about.html?app=1',
    ]) {
      expect(
        top('http://127.0.0.1:7999$path'),
        NavigationDecision.navigate,
        reason: path,
      );
    }
    expect(opened, isEmpty);
  });

  test('anything else at the top level is refused', () {
    for (final url in [
      'about:blank',
      'http://127.0.0.1:7999/a/abcdefghjkmn',
      'http://127.0.0.1:7999/a/x?app=1',
      'http://127.0.0.1:7999/',
      'http://127.0.0.1:7999/api/token?app=1',
      'http://abc.localhost:7999/index.html',
      'http://localhost:7999/a/abcdefghjkmn?app=1',
      'http://127.0.0.1:7480/a/abcdefghjkmn?app=1',
      'file:///etc/passwd',
      'javascript:alert(1)',
    ]) {
      expect(top(url), NavigationDecision.prevent, reason: url);
    }
    expect(opened, isEmpty, reason: 'none of those is a web link outside');
  });

  test('a web or mail link goes outside the app', () {
    expect(top('https://example.com/x'), NavigationDecision.prevent);
    expect(top('mailto:a@example.com'), NavigationDecision.prevent);
    expect(opened, [
      Uri.parse('https://example.com/x'),
      Uri.parse('mailto:a@example.com'),
    ]);
  });

  test('the top level stays on its own artifact: another artifact\'s app '
      'view is refused and handed to openArtifact', () {
    final others = <String>[];
    NavigationDecision as(String url) => WebViewAppSurface.decide(
      base,
      url,
      isMainFrame: true,
      own: 'abcdefghjkmn',
      openExternal: opened.add,
      openArtifact: others.add,
    );
    expect(as('$base/a/abcdefghjkmn/v/2?app=1'), NavigationDecision.navigate);
    expect(as('$base/a/zyxwvtsrqpnm?app=1'), NavigationDecision.prevent);
    expect(others, ['zyxwvtsrqpnm']);
    expect(opened, isEmpty);
  });

  test('on iOS the WebView is inspectable outside release builds, and '
      'WebKit\'s swipe back and forward stay off', () {
    expect(iosWebKitSettings(), (inspectable: true, backForwardGestures: false));
    expect(
      iosWebKitSettings(release: true),
      (inspectable: false, backForwardGestures: false),
    );
  });

  test('frames inside the page navigate freely', () {
    expect(
      WebViewAppSurface.decide(
        base,
        'http://abc.localhost:7999/index.html',
        isMainFrame: false,
      ),
      NavigationDecision.navigate,
    );
  });

  group('on the core', () {
    const aid = '7q3k9mzx2b4t';
    final origins = AppOrigins.forPlatform(android: true);
    final shellUrl = Uri.parse('${origins.shell}/a/$aid?app=1');
    late FakeCore core;
    late List<String> ran;
    late List<String> shellMessages;
    late List<String> pins;
    late CoreShellLink link;

    setUp(() {
      core = FakeCore()
        ..on('GET', '/a/{aid}', (r, p) {
          return CoreResponse(200, const [
            ('content-type', 'text/html; charset=utf-8'),
          ], utf8.encode('<html><head></head><body></body></html>'));
        })
        ..on('GET', '/api/x', (r, p) => FakeCore.json({'x': 1}));
      ran = [];
      shellMessages = [];
      pins = [];
      link = CoreShellLink(
        core,
        origins,
        runJs: (js) async => ran.add(js),
        onShellMessage: shellMessages.add,
        onPinned: pins.add,
      );
    });

    tearDown(() => link.dispose());

    /// Answers the shell document to navigation [n]; its nonce.
    Future<String> serve(int n) async {
      final a = await link.request(
        SchemeRequest(
          url: shellUrl,
          method: 'GET',
          headers: const {},
          mainFrame: true,
          navigation: n,
        ),
      );
      expect(a.status, 200);
      final meta = RegExp(r'<meta name="clax-nonce" content="([^"]*)">')
          .firstMatch(utf8.decode(a.body))![1]!;
      expect(meta, a.nonce);
      return meta;
    }

    String msg(
      String type,
      String nonce, [
      Map<String, Object?> more = const {},
    ]) => jsonEncode({'type': type, 'nonce': nonce, ...more});

    String call(String nonce, int id) => msg('call', nonce, {
      'id': id,
      'method': 'GET',
      'path': '/api/x',
      'headers': const <Object?>[],
      'body': null,
      'body64': null,
    });

    test('two shell documents answered under one number both join its set; '
        'the first the main frame presents after the commit pins the load, '
        'and a message under the other is then dropped', () async {
      final a = await serve(1);
      final b = await serve(1);
      expect(a, isNot(b));
      link.committed(shellUrl, 1);
      link.channel(msg('ready', b));
      expect(pins, [b]);
      expect(link.nonce, b);
      expect(shellMessages, [msg('ready', b)]);
      link.channel(msg('ready', a));
      link.channel(call(a, 1));
      await pumpEventQueue();
      expect(shellMessages, [msg('ready', b)]);
      expect(core.requests.where((r) => r.path == '/api/x'), isEmpty);
      expect(pins, [b]);
    });

    test('the relay answers the load under its nonce', () async {
      final n = await serve(1);
      link.committed(shellUrl, 1);
      link.channel(call(n, 1));
      await pumpEventQueue();
      expect(core.requests.last.path, '/api/x');
      expect(core.requests.last.as, CallAs.owner);
      expect(
        ran.single,
        startsWith('window.claxTransport.callResult("$n", 1, '),
      );
      expect(shellMessages, isEmpty, reason: 'the relay\'s, not the shell\'s');
    });

    test('a message before the pin under a nonce of another number is '
        'dropped and pins nothing', () async {
      final first = await serve(1);
      // A second request while the first navigation is in flight.
      final second = await serve(2);
      link.committed(shellUrl, 1);
      link.channel(msg('ready', second));
      expect(pins, isEmpty);
      expect(shellMessages, isEmpty);
      link.channel(msg('ready', first));
      expect(pins, [first]);
    });

    test('a commit drops the sets of every number up to its own', () async {
      final one = await serve(1);
      await serve(2);
      link.committed(shellUrl, 2);
      link.committed(shellUrl, 1);
      link.channel(msg('ready', one));
      expect(pins, isEmpty);
    });

    test('a set whose navigation is dropped is gone', () async {
      final n = await serve(1);
      link
        ..dropped(1)
        ..committed(shellUrl, 1);
      link.channel(msg('ready', n));
      expect(pins, isEmpty);
      expect(link.nonce, isNull);
    });

    test('a later minted nonce whose navigation does not commit leaves the '
        'load\'s unchanged', () async {
      final n = await serve(1);
      link.committed(shellUrl, 1);
      link.channel(msg('ready', n));
      final later = await serve(2);
      link.channel(msg('ready', later));
      link.dropped(2);
      link.channel(msg('ready', n));
      expect(link.nonce, n);
      expect(pins, [n]);
      expect(shellMessages, [msg('ready', n), msg('ready', n)]);
    });

    test('a committed with no number takes none and leaves the load without '
        'a channel', () async {
      final n = await serve(1);
      link.committed(shellUrl, null);
      link.channel(msg('ready', n));
      link.channel(call(n, 1));
      await pumpEventQueue();
      expect(pins, isEmpty);
      expect(core.requests.where((r) => r.path == '/api/x'), isEmpty);
    });

    test('a new commit ends the last load\'s relay: its stream is cancelled '
        'and its calls go unanswered', () async {
      final gate = Completer<CoreResponse>();
      core.on('GET', '/api/slow', (r, p) => gate.future);
      final n = await serve(1);
      link.committed(shellUrl, 1);
      link.channel(msg('subscribe', n, {'id': 1}));
      link.channel(
        msg('call', n, {
          'id': 2,
          'method': 'GET',
          'path': '/api/slow',
          'headers': const <Object?>[],
        }),
      );
      await pumpEventQueue();
      expect(core.subscribers, 1);
      ran.clear();
      await serve(2);
      link.committed(shellUrl, 2);
      expect(core.subscribers, 0);
      gate.complete(FakeCore.json({}));
      await pumpEventQueue();
      expect(ran, isEmpty);
    });

    test('no request without a navigation number mints a nonce', () async {
      final a = await link.request(
        SchemeRequest(
          url: shellUrl,
          method: 'GET',
          headers: const {},
          mainFrame: false,
        ),
      );
      expect(a.status, 403);
      expect(a.nonce, isNull);
    });

    test('a malformed message is dropped', () {
      for (final raw in ['nope', '[]', '{"type":1}', '{}']) {
        link.channel(raw);
      }
      expect(shellMessages, isEmpty);
    });

    test('the surface attaches the scheme host again after each '
        'setNavigationDelegate, before it loads', () async {
      final log = <String>[];
      final host = _RecordingHost(log);
      final surface = SchemeAttachment(host, () => 7);
      Future<void> set() async => log.add('delegate');
      await surface.setDelegate(set, link);
      await surface.setDelegate(set, link);
      expect(log, ['delegate', 'attach 7', 'delegate', 'attach 7']);
      await surface.attached;
      surface.detach();
      expect(log.last, 'detach 7');
    });
  });
}

class _RecordingHost implements SchemeHostPort {
  _RecordingHost(this.log);

  final List<String> log;

  @override
  Future<bool> attach(int webViewId, SchemeClient client) async {
    log.add('attach $webViewId');
    return true;
  }

  @override
  Future<void> detach(int webViewId) async => log.add('detach $webViewId');
}
