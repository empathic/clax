import 'dart:async';
import 'dart:convert';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/core/channel_relay.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/core/scheme.dart';
import 'package:clax_app/core/scheme_worker.dart';
import 'package:clax_app/core/shell_link.dart';
import 'package:clax_app/ui/host_protocol.dart' show HomeDeviceEvent, HostMessage, Ready;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';
import '../support/fake_scheme_native.dart';

const _aid = '7q3k9mzx2b4t';
final _origins = AppOrigins.forPlatform(android: true);
final _shellUrl = '${_origins.shell}/a/$_aid?app=1';
final _pageUrl = '${_origins.content(_aid)}/v/1/app.js';

/// The worker isolate's name, as [SchemeWorker] spawns it.
const _workerName = 'clax scheme worker';

/// A core call on any isolate but the scheme worker's fails: the UI
/// isolate makes none.
void _onlyInWorker() {
  final name = Isolate.current.debugName;
  if (name != _workerName) throw StateError('a core call on $name');
}

/// The core both sides answer from: a shell document, a page's script,
/// and an API call.
FakeCore _core({bool guarded = true}) {
  final core = FakeCore(unrouted: (_) {});
  void guard() {
    if (guarded) _onlyInWorker();
  }

  core
    ..on('GET', '/a/{aid}', (r, p) {
      guard();
      return CoreResponse(
        200,
        const [
          ('content-type', 'text/html; charset=utf-8'),
          ('cache-control', 'private, no-cache'),
          ('access-control-allow-origin', '*'),
        ],
        utf8.encode('<html><head><title>s</title></head><body></body></html>'),
      );
    })
    ..on('GET', '/v/1/app.js', (r, p) {
      guard();
      return CoreResponse(200, const [
        ('content-type', 'text/javascript'),
        ('cache-control', 'public, max-age=31536000, immutable'),
        ('etag', '"0123456789abcdef"'),
      ], Uint8List.fromList(List.generate(200000, (i) => 32 + i % 90)));
    })
    ..on('GET', '/v/1/missing.js', (r, p) {
      guard();
      return CoreResponse(404, const [
        ('content-type', 'application/json'),
      ], utf8.encode('{"error":{"code":"not_found"}}'));
    })
    ..on('GET', '/api/artifacts', (r, p) {
      guard();
      return CoreResponse(200, const [
        ('content-type', 'application/json'),
      ], utf8.encode(jsonEncode({'artifacts': List.filled(3000, 'x' * 50)})));
    });
  return core;
}

CoreCalls _workerCore(int handle) => _core();

/// The headers and body a scheme answer goes out with, as `respond` sends
/// them; the shell document's nonce made the same for comparing.
String _shape(int status, Map<Object?, Object?> headers, Uint8List body) {
  final names = [for (final k in headers.keys) '$k']..sort();
  final body8 = latin1
      .decode(body)
      .replaceAll(RegExp(r'content="[A-Za-z0-9_-]{32}"'), 'content="N"');
  return '$status '
      '${[for (final n in names) '$n: ${headers[n]}'].join('; ')}\n'
      '$body8';
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late FakeSchemeNative native;
  late SchemeWorker worker;

  setUp(() {
    native = FakeSchemeNative();
    worker = SchemeWorker.start(
      coreHandle: 1,
      origins: _origins,
      attach: _workerCore,
      messenger: native.messenger,
    );
  });

  tearDown(() {
    worker.close();
    native.close();
  });

  test('the guard fails a core call made on this isolate', () {
    expect(_onlyInWorker, throwsStateError);
  });

  test('the worker answers each scheme request with the bytes and headers '
      'the link on this isolate answers, and this isolate makes no core '
      'call', () async {
    final messages = <HostMessage>[], pins = <String>[], faults = <String>[];
    final link = worker.link(
      onMessage: (_, m) => messages.add(m),
      onPinned: pins.add,
      onFault: faults.add,
    );
    expect(await link.attach(7), isTrue);
    expect(native.attached, [7]);

    final here = CoreShellLink(
      _core(guarded: false),
      _origins,
      runJs: (_) async {},
      onShellMessage: (_) {},
      onPinned: (_) {},
      onFault: (_) {},
    );
    final requests = [
      (_shellUrl, true, 1),
      (_pageUrl, false, null),
      ('${_origins.shell}/a/$_aid?app=1', false, null),
      ('${_origins.content(_aid)}/v/1/missing.js', false, null),
    ];
    var reqId = 0;
    for (final (url, mainFrame, navigation) in requests) {
      final id = ++reqId;
      native.emit('request', {
        'webViewId': 7,
        'reqId': id,
        'url': url,
        'method': 'GET',
        'headers': {'Accept': '*/*', 'If-None-Match': '"x"'},
        'mainFrame': mainFrame,
        'navigation': ?navigation,
      });
      final sent = await native.until(() => native.responds[id]);
      final want = await here.request(
        SchemeRequest(
          url: Uri.parse(url),
          method: 'GET',
          headers: const {'Accept': '*/*', 'If-None-Match': '"x"'},
          mainFrame: mainFrame,
          navigation: navigation,
        ),
      );
      expect(
        _shape(
          sent['status']! as int,
          sent['headers']! as Map,
          sent['body']! as Uint8List,
        ),
        _shape(want.status, want.headers, want.body),
        reason: url,
      );
    }
    expect(faults, isEmpty);
  });

  test('a load\'s nonce is pinned in the worker and its calls relayed as '
      'the relay on this isolate relays them', () async {
    final messages = <HostMessage>[], pins = <String>[];
    final link = worker.link(
      onMessage: (_, m) => messages.add(m),
      onPinned: pins.add,
      onFault: (_) {},
    );
    await link.attach(7);
    native.emit('request', {
      'webViewId': 7,
      'reqId': 1,
      'url': _shellUrl,
      'method': 'GET',
      'headers': <String, String>{},
      'mainFrame': true,
      'navigation': 1,
    });
    final doc = await native.until(() => native.responds[1]);
    final nonce = RegExp(
      r'clax-nonce" content="([^"]+)"',
    ).firstMatch(utf8.decode(doc['body']! as Uint8List))![1]!;
    native
      ..emit('committed', {'webViewId': 7, 'url': _shellUrl, 'navigation': 1})
      ..emit('channel', {
        'webViewId': 7,
        'message': jsonEncode({
          'type': 'ready',
          'nonce': nonce,
          'artifact': _aid,
          'version': 1,
          'latest': 1,
          'file': 'index.html',
        }),
      });
    final call = {
      'type': 'call',
      'nonce': nonce,
      'id': 3,
      'method': 'GET',
      'path': '/api/artifacts',
      'headers': [
        ['accept', 'application/json'],
      ],
    };
    native.emit('channel', {'webViewId': 7, 'message': jsonEncode(call)});
    await native.until(() => native.evals.isEmpty ? null : native.evals);
    final ran = <String>[];
    ChannelRelay(
      _core(guarded: false),
      runJs: (js) async => ran.add(js),
    ).handle(call);
    await pumpEventQueue();
    expect(native.evals, ran);
    expect(native.evals.single, startsWith('window.claxTransport.callResult('));
    await pumpEventQueue();
    expect(pins, [nonce]);
    expect(link.nonce, nonce);
    // The non-relay message reaches this isolate; the call does not.
    expect(messages.map((m) => m.runtimeType), [Ready]);
    // A commit clears the mirrored nonce until the next pin.
    native.emit('committed', {'webViewId': 7, 'url': _shellUrl});
    for (var i = 0; i < 100 && link.nonce != null; i++) {
      await pumpEventQueue();
    }
    expect(link.nonce, isNull);
  });

  test('a host call runs as the script it writes, written in the worker '
      'and run by the native side', () async {
    final link = worker.link(
      onMessage: (_, _) {},
      onPinned: (_) {},
      onFault: (_) {},
    );
    await link.attach(7);
    final call = HomeDeviceEvent('apps', [
      {'id': _aid, 'title': 'Runs', 'icon': {'emoji': '🏃'}},
    ]);
    await link.run(call);
    expect(native.evals, [call.toJs()]);
    link.dispose();
    await link.run(call);
    expect(native.evals, hasLength(1), reason: 'nothing runs once disposed');
  });

  test('a shell message that is no message of its type reaches nothing, '
      'and the worker goes on', () async {
    final messages = <HostMessage>[];
    final link = worker.link(
      onMessage: (_, m) => messages.add(m),
      onPinned: (_) {},
      onFault: (_) {},
    );
    await link.attach(7);
    native.emit('request', {
      'webViewId': 7,
      'reqId': 1,
      'url': _shellUrl,
      'method': 'GET',
      'headers': <String, String>{},
      'mainFrame': true,
      'navigation': 1,
    });
    final doc = await native.until(() => native.responds[1]);
    final nonce = RegExp(
      r'clax-nonce" content="([^"]+)"',
    ).firstMatch(utf8.decode(doc['body']! as Uint8List))![1]!;
    native
      ..emit('committed', {'webViewId': 7, 'url': _shellUrl, 'navigation': 1})
      // A ready without its fields.
      ..emit('channel', {
        'webViewId': 7,
        'message': jsonEncode({'type': 'ready', 'nonce': nonce}),
      })
      ..emit('request', {
        'webViewId': 7,
        'reqId': 2,
        'url': _pageUrl,
        'method': 'GET',
        'headers': <String, String>{},
        'mainFrame': false,
      });
    expect((await native.until(() => native.responds[2]))['status'], 200);
    expect(messages, isEmpty);
  });

  test('a request of a WebView no link attached is answered 503', () async {
    final link = worker.link(
      onMessage: (_, _) {},
      onPinned: (_) {},
      onFault: (_) {},
    );
    await link.attach(7);
    native.emit('request', {
      'webViewId': 8,
      'reqId': 1,
      'url': _pageUrl,
      'method': 'GET',
      'headers': <String, String>{},
      'mainFrame': false,
    });
    final r = await native.until(() => native.responds[1]);
    expect(r['status'], 503);
  });

  group('the worker\'s transport', () {
    late SchemeWorker apiWorker;

    setUp(() {
      apiWorker = SchemeWorker.start(
        coreHandle: 1,
        origins: _origins,
        attach: _apiCore,
        messenger: native.messenger,
      );
    });

    tearDown(() => apiWorker.close());

    test('answers HttpClaxApi as CoreTransport here does, the JSON decoded '
        'in the worker, with no core call here', () async {
      final api = HttpClaxApi.over(apiWorker.transport());
      final here = HttpClaxApi.over(CoreTransport(_apiCore(0, guarded: false)));
      final listed = await api.listArtifacts();
      expect(
        [for (final a in listed) (a.id, a.title)],
        [for (final a in await here.listArtifacts()) (a.id, a.title)],
      );
      expect(listed.map((a) => a.title), ['Runs']);
      final raw = await apiWorker.transport().send(
        'GET',
        '/api/artifacts',
        timeout: const Duration(seconds: 10),
      );
      expect(raw.status, 200);
      expect(raw.json, jsonDecode(utf8.decode(raw.body)));
      await expectLater(
        api.getArtifact('0000000000zz'),
        throwsA(isA<ClaxApiError>().having((e) => e.status, 'status', 404)),
      );
    });

    test('a call as the person reaches the core as the owner\'s browser; '
        'every other as the app', () async {
      final t = apiWorker.transport();
      Future<Object?> asOf({required bool person}) async => (await t.send(
        'GET',
        '/api/as',
        timeout: const Duration(seconds: 10),
        person: person,
      )).json?['as'];
      expect(await asOf(person: true), 'owner-browser');
      expect(await asOf(person: false), 'owner');
    });

    test('streams the core\'s events through the worker', () async {
      final stream = apiWorker.transport().sse();
      final first = await stream.first.timeout(const Duration(seconds: 10));
      expect(first.event, 'ready');
    });

    test('a call in flight when the worker closes fails as a closed '
        'transport\'s, and later ones too', () async {
      final t = apiWorker.transport();
      final held = t.send(
        'GET',
        '/api/held',
        timeout: const Duration(minutes: 1),
      );
      // Let the call reach the worker before it closes.
      for (var i = 0; i < 20; i++) {
        await pumpEventQueue();
      }
      apiWorker.close();
      Matcher closed() => throwsA(
        isA<ClaxApiError>().having((e) => e.body['reason'], 'reason', 'closed'),
      );
      await expectLater(held, closed());
      await expectLater(
        t.send('GET', '/api/artifacts', timeout: const Duration(seconds: 1)),
        closed(),
      );
    });
  });
}

/// A core whose routes are a [FakeClaxApi]'s with one app, and a call that
/// never answers (`/api/held`); guarded as [_core] is.
CoreCalls _apiCore(int handle, {bool guarded = true}) {
  final api = FakeClaxApi()..seedArtifact('Runs', {'index.html': '<p>r</p>'});
  final core = FakeCore.fromApi(api, unrouted: (_) {});
  final held = FakeCore(unrouted: (_) {})
    ..on('GET', '/api/held', (r, p) => Completer<CoreResponse>().future)
    // Who the call speaks as, as the core reads it.
    ..on('GET', '/api/as', (r, p) async => FakeCore.json({'as': r.as.wire}));
  return _Guarded(core, held, guarded);
}

class _Guarded implements CoreCalls {
  _Guarded(this.core, this.held, this.guarded);

  final FakeCore core, held;
  final bool guarded;

  @override
  CoreCall call(CoreRequest r) {
    if (guarded) _onlyInWorker();
    return r.path == '/api/held' || r.path == '/api/as'
        ? held.call(r)
        : core.call(r);
  }

  @override
  void cancel(int tag) => core.cancel(tag);

  @override
  CoreSubscription subscribe({String? lastEventId}) {
    if (guarded) _onlyInWorker();
    return core.subscribe(lastEventId: lastEventId);
  }
}
