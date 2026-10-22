import 'dart:async';
import 'dart:convert';

import 'package:clax_app/core/scheme.dart';
import 'package:clax_app/core/scheme_host.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

class _Client implements SchemeClient {
  final List<SchemeRequest> requests = [];
  final List<String> messages = [];
  final List<(Uri?, int?)> commits = [];
  final List<int> drops = [];
  final List<Uri?> refusals = [];
  Future<SchemeAnswer> Function(SchemeRequest r) answer = (r) async =>
      SchemeAnswer(200, const {
        'content-type': 'text/plain',
      }, utf8.encode('ok'));

  @override
  Future<SchemeAnswer> request(SchemeRequest r) {
    requests.add(r);
    return answer(r);
  }

  @override
  void channel(String message) => messages.add(message);

  @override
  void committed(Uri? url, int? navigation) => commits.add((url, navigation));

  @override
  void dropped(int navigation) => drops.add(navigation);

  @override
  void refused(Uri? url) => refusals.add(url);
}

/// The runner's scheme host as the work channel shows it: events queued
/// until a `next` takes them, a `next` kept until there is one.
class _Native {
  final List<Map<String, Object?>> queued = [];
  Completer<List<Object?>?>? parked;
  int nexts = 0;

  void emit(String event, Map<String, Object?> args) {
    final e = {...args, 'event': event};
    if (parked case final p?) {
      parked = null;
      p.complete([e]);
    } else {
      queued.add(e);
    }
  }

  Future<Object?> next() {
    nexts++;
    if (queued.isNotEmpty) {
      final taken = [...queued];
      queued.clear();
      return Future.value(taken);
    }
    return (parked = Completer()).future;
  }

  void release() {
    parked?.complete(null);
    parked = null;
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('test/scheme');
  const work = MethodChannel('test/scheme-worker');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late List<MethodCall> sent;
  late _Native native;
  late SchemeHost host;

  setUp(() {
    sent = [];
    native = _Native();
    messenger.setMockMethodCallHandler(channel, (c) async {
      sent.add(c);
      return c.method == 'attach' ? true : null;
    });
    messenger.setMockMethodCallHandler(work, (c) async {
      switch (c.method) {
        case 'next':
          return native.next();
        case 'release':
          native.release();
          return null;
      }
      sent.add(c);
      return null;
    });
    host = SchemeHost(channel: channel, work: work);
  });

  tearDown(() {
    host.dispose();
    messenger.setMockMethodCallHandler(channel, null);
    messenger.setMockMethodCallHandler(work, null);
  });

  /// Queues [method] on the native side, as the runner's SchemeHost does,
  /// and lets the host take it.
  Future<void> fromNative(String method, Map<String, Object?> args) async {
    native.emit(method, args);
    await pumpEventQueue();
  }

  test('attach and detach name the WebView', () async {
    final c = _Client();
    expect(await host.attach(3, c), isTrue);
    await host.detach(3);
    expect(sent.map((c) => c.method), ['attach', 'detach']);
    expect(sent.map((c) => c.arguments), [
      {'webViewId': 3},
      {'webViewId': 3},
    ]);
  });

  test(
    'a request goes to its WebView\'s client and its answer to respond',
    () async {
      final c = _Client();
      await host.attach(3, c);
      sent.clear();
      await fromNative('request', {
        'webViewId': 3,
        'reqId': 41,
        'url': 'https://app.clax.invalid/a/7q3k9mzx2b4t?app=1',
        'method': 'GET',
        'headers': {'Accept': 'text/html'},
        'mainFrame': true,
        'navigation': 2,
      });
      await pumpEventQueue();
      final r = c.requests.single;
      expect(r.url, Uri.parse('https://app.clax.invalid/a/7q3k9mzx2b4t?app=1'));
      expect(r.method, 'GET');
      expect(r.headers, {'Accept': 'text/html'});
      expect(r.mainFrame, isTrue);
      expect(r.navigation, 2);
      final respond = sent.single;
      expect(respond.method, 'respond');
      expect(respond.arguments, {
        'reqId': 41,
        'status': 200,
        'headers': {'content-type': 'text/plain'},
        'body': Uint8List.fromList(utf8.encode('ok')),
      });
    },
  );

  test('a request of no attached WebView, or whose answer fails, is '
      'answered at once', () async {
    final c = _Client()..answer = (r) => Future.error(StateError('x'));
    await host.attach(3, c);
    sent.clear();
    await fromNative('request', {
      'webViewId': 9,
      'reqId': 1,
      'url': 'https://app.clax.invalid/_clax/a.js',
      'method': 'GET',
      'headers': <String, String>{},
      'mainFrame': false,
    });
    await fromNative('request', {
      'webViewId': 3,
      'reqId': 2,
      'url': 'https://app.clax.invalid/_clax/a.js',
      'method': 'GET',
      'headers': <String, String>{},
      'mainFrame': false,
    });
    await pumpEventQueue();
    expect(
      sent.map(
        (c) => ((c.arguments as Map)['reqId'], (c.arguments as Map)['status']),
      ),
      [(1, 503), (2, 500)],
    );
  });

  test(
    'channel, committed, dropped and refused reach their WebView\'s client',
    () async {
      final c = _Client();
      final other = _Client();
      await host.attach(3, c);
      await host.attach(4, other);
      await fromNative('channel', {'webViewId': 3, 'message': '{"type":"x"}'});
      await fromNative('committed', {
        'webViewId': 3,
        'url': 'clax://app/a/7q3k9mzx2b4t?app=1',
        'navigation': 5,
      });
      await fromNative('committed', {'webViewId': 3, 'url': 'about:blank'});
      await fromNative('dropped', {'webViewId': 3, 'navigation': 6});
      await fromNative('refused', {
        'webViewId': 3,
        'url': 'https://app.clax.invalid/',
      });
      expect(c.messages, ['{"type":"x"}']);
      expect(c.commits, [
        (Uri.parse('clax://app/a/7q3k9mzx2b4t?app=1'), 5),
        (Uri.parse('about:blank'), null),
      ]);
      expect(c.drops, [6]);
      expect(c.refusals, [Uri.parse('https://app.clax.invalid/')]);
      expect(other.messages, isEmpty);
      expect(other.commits, isEmpty);
    },
  );

  test('the host takes events only once a WebView is attached, one next at '
      'a time, and a disposed host releases the waiting next', () async {
    await pumpEventQueue();
    expect(native.nexts, 0, reason: 'nothing attached yet');
    final c = _Client();
    await host.attach(3, c);
    await pumpEventQueue();
    expect(native.nexts, 1);
    await fromNative('channel', {'webViewId': 3, 'message': 'a'});
    await fromNative('channel', {'webViewId': 3, 'message': 'b'});
    expect(c.messages, ['a', 'b']);
    expect(native.nexts, 3);
    expect(native.parked, isNotNull);
    host.dispose();
    await pumpEventQueue();
    expect(native.parked, isNull);
    await fromNative('channel', {'webViewId': 3, 'message': 'c'});
    expect(c.messages, ['a', 'b']);
  });

  test('a detached WebView hears nothing more', () async {
    final c = _Client();
    await host.attach(3, c);
    await host.detach(3);
    await fromNative('channel', {'webViewId': 3, 'message': 'x'});
    expect(c.messages, isEmpty);
  });

  test('SchemeAttachment attaches after each delegate and detaches the '
      'WebView it attached', () async {
    final c = _Client();
    final a = SchemeAttachment((id) => host.attach(id, c), host.detach, () => 8);
    var delegates = 0;
    await a.setDelegate(() async => delegates++);
    await a.setDelegate(() async => delegates++);
    expect(await a.attached, isTrue);
    a.detach();
    await pumpEventQueue();
    expect(delegates, 2);
    expect(sent.map((c) => c.method), ['attach', 'attach', 'detach']);
  });
}
