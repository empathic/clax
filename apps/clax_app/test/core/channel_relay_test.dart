import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/core/channel_relay.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/frame.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';

const _nonce = 'n0nce-of-the-load_0123456789';

/// [FakeCore] that also records the resume point of each subscription.
class _Core implements CoreCalls {
  _Core(this.fake);

  final FakeCore fake;
  final List<String?> resumedFrom = [];

  @override
  CoreCall call(CoreRequest r) => fake.call(r);

  @override
  void cancel(int tag) => fake.cancel(tag);

  @override
  CoreSubscription subscribe({String? lastEventId}) {
    resumedFrom.add(lastEventId);
    return fake.subscribe(lastEventId: lastEventId);
  }
}

/// One `window.claxTransport.<method>(…)` the relay ran: its method and
/// its arguments, parsed.
typedef _Ran = (String, List<Object?>);

_Ran _parse(String js) {
  final m = RegExp(
    r'^window\.claxTransport\.(\w+)\((.*)\)$',
    dotAll: true,
  ).firstMatch(js);
  if (m == null) throw TestFailure('not a claxTransport call: $js');
  return (m[1]!, jsonDecode('[${m[2]}]') as List<Object?>);
}

void main() {
  late FakeCore fake;
  late _Core core;
  late List<String> ran;
  late ChannelRelay relay;

  setUp(() {
    fake = FakeCore();
    core = _Core(fake);
    ran = [];
    relay = ChannelRelay(
      core,
      runJs: (js) async {
        ran.add(js);
      },
    );
  });

  tearDown(() => relay.dispose());

  List<_Ran> calls() => ran.map(_parse).toList();

  Map<String, Object?> call(
    int id,
    String method,
    String path, {
    Object? body,
    Object? body64,
    List<List<String>> headers = const [],
    bool chunked = false,
  }) => {
    'type': 'call',
    'nonce': _nonce,
    'id': id,
    'method': method,
    'path': path,
    'headers': headers,
    'body': body,
    'body64': body64,
    if (chunked) 'chunked': true,
  };

  /// Lets the fake core's answers and the relay's continuations run.
  Future<void> settle() => pumpEventQueue();

  test('a call goes to the core as the owner and is answered by '
      'callResult under the call\'s nonce, a JSON body as text', () async {
    fake.on('POST', '/api/artifacts/{aid}/threads', (r, p) {
      return FakeCore.json({'ok': p['aid']}, status: 201);
    });
    expect(
      relay.handle(
        call(
          17,
          'POST',
          '/api/artifacts/abc/threads?x=1',
          body: '{"a":1}',
          headers: [
            ['content-type', 'application/json'],
            ['Host', 'evil'],
          ],
        ),
      ),
      isTrue,
    );
    await settle();
    final r = fake.requests.single;
    expect(r.as, CallAs.owner);
    expect(r.method, 'POST');
    expect(r.path, '/api/artifacts/abc/threads?x=1');
    expect(r.headers, [('content-type', 'application/json')]);
    expect(utf8.decode(r.body!), '{"a":1}');
    expect(ran, [
      'window.claxTransport.callResult("$_nonce", 17, '
          '{"status":201,"headers":[["content-type","application/json"]],'
          '"body":"{\\"ok\\":\\"abc\\"}"})',
    ]);
  });

  test('a body64 call reaches the core as its bytes', () async {
    fake.on('PUT', '/api/x', (r, p) => FakeCore.json({}));
    relay.handle(call(1, 'PUT', '/api/x', body64: base64Encode([0, 255, 1])));
    await settle();
    expect(fake.requests.single.body, [0, 255, 1]);
  });

  test('a binary answer goes as body64', () async {
    fake.on(
      'GET',
      '/api/clip.png',
      (r, p) => CoreResponse(200, const [
        ('content-type', 'image/png'),
      ], Uint8List.fromList([137, 80, 78, 71])),
    );
    relay.handle(call(2, 'GET', '/api/clip.png'));
    await settle();
    final (method, args) = calls().single;
    expect(method, 'callResult');
    expect(args, [
      _nonce,
      2,
      {
        'status': 200,
        'headers': [
          ['content-type', 'image/png'],
        ],
        'body64': base64Encode([137, 80, 78, 71]),
      },
    ]);
  });

  test('a text answer that is not valid UTF-8 goes as body64', () async {
    fake.on(
      'GET',
      '/api/x',
      (r, p) => CoreResponse(200, const [
        ('content-type', 'text/plain'),
      ], Uint8List.fromList([0xff, 0xfe])),
    );
    relay.handle(call(3, 'GET', '/api/x'));
    await settle();
    final (_, args) = calls().single;
    expect((args[2]! as Map)['body64'], base64Encode([0xff, 0xfe]));
  });

  test(
    'a 3 MiB answer goes as a head, then in three callResultChunk calls',
    () async {
      final bytes = Uint8List(3 << 20);
      for (var i = 0; i < bytes.length; i++) {
        bytes[i] = i % 251;
      }
      fake.on(
        'GET',
        '/api/big',
        (r, p) => CoreResponse(200, const [
          ('content-type', 'application/octet-stream'),
        ], bytes),
      );
      relay.handle(call(4, 'GET', '/api/big'));
      await settle();
      final got = calls();
      expect(got.map((c) => c.$1), [
        'callResult',
        'callResultChunk',
        'callResultChunk',
        'callResultChunk',
      ]);
      expect(got.first.$2, [
        _nonce,
        4,
        {
          'status': 200,
          'headers': [
            ['content-type', 'application/octet-stream'],
          ],
          'chunked': true,
        },
      ]);
      final joined = <int>[];
      for (final (i, (_, args)) in got.skip(1).indexed) {
        expect(args.sublist(0, 4), [_nonce, 4, i, i == 2]);
        final part = base64Decode(args[4]! as String);
        expect(part, hasLength(1 << 20));
        joined.addAll(part);
      }
      expect(joined, bytes);
    },
  );

  test('a text answer over 1 MiB goes in chunks too', () async {
    final text = 'a' * ((1 << 20) + 1);
    fake.on(
      'GET',
      '/api/t',
      (r, p) => CoreResponse(200, const [
        ('content-type', 'text/plain'),
      ], utf8.encode(text)),
    );
    relay.handle(call(5, 'GET', '/api/t'));
    await settle();
    expect(calls().map((c) => c.$1), [
      'callResult',
      'callResultChunk',
      'callResultChunk',
    ]);
  });

  test('a chunked call waits for its callChunks and sends their bytes in '
      'order', () async {
    fake.on('POST', '/api/up', (r, p) => FakeCore.json({}));
    relay.handle(call(6, 'POST', '/api/up', chunked: true));
    relay.handle({
      'type': 'callChunk',
      'nonce': _nonce,
      'id': 6,
      'n': 0,
      'last': false,
      'body64': base64Encode([1, 2]),
    });
    await settle();
    expect(fake.requests, isEmpty);
    // Out of order: dropped.
    relay.handle({
      'type': 'callChunk',
      'nonce': _nonce,
      'id': 6,
      'n': 2,
      'last': true,
      'body64': base64Encode([9]),
    });
    relay.handle({
      'type': 'callChunk',
      'nonce': _nonce,
      'id': 6,
      'n': 1,
      'last': true,
      'body64': base64Encode([3]),
    });
    await settle();
    expect(fake.requests.single.body, [1, 2, 3]);
    expect(calls().single.$1, 'callResult');
  });

  test('a callAbort before the answer runs no JS and cancels the core\'s '
      'call', () async {
    final gate = Completer<CoreResponse>();
    fake.on('GET', '/api/slow', (r, p) => gate.future);
    relay.handle(call(7, 'GET', '/api/slow'));
    await settle();
    expect(
      relay.handle({'type': 'callAbort', 'nonce': _nonce, 'id': 7}),
      isTrue,
    );
    gate.complete(FakeCore.json({}));
    await settle();
    expect(ran, isEmpty);
    expect(fake.cancelled.single.path, '/api/slow');
  });

  test('a call the core cannot answer is answered with its error', () async {
    fake.close();
    relay.handle(call(8, 'GET', '/api/x'));
    await settle();
    final (_, args) = calls().single;
    final r = args[2]! as Map;
    expect(r['status'], 503);
    expect(jsonDecode(r['body']! as String), {
      'error': {'code': 'core_closed', 'message': 'the core is closed'},
    });
  });

  test('subscribe forwards pushed blocks in order, the end block before '
      'streamEnd', () async {
    expect(
      relay.handle({'type': 'subscribe', 'nonce': _nonce, 'id': 9}),
      isTrue,
    );
    await settle();
    fake.push('event: x\ndata: 1\n\n');
    fake.push('event: x\ndata: 2\n\n');
    fake.end('stream_refused', status: 503);
    await settle();
    expect(core.resumedFrom, [null]);
    final got = calls();
    expect(got.map((c) => c.$1), [
      'streamBlock',
      'streamBlock',
      'streamBlock',
      'streamBlock',
      'streamEnd',
    ]);
    expect(got.map((c) => c.$2.sublist(0, 2)), everyElement([_nonce, 9]));
    expect(got.map((c) => c.$2.length > 2 ? c.$2[2] : null), [
      FakeCore.readyBlock('fake-1'),
      'event: x\ndata: 1\n\n',
      'event: x\ndata: 2\n\n',
      endBlock('stream_refused', status: 503),
      null,
    ]);
  });

  test(
    'subscribe with lastEventId opens the core subscription from it',
    () async {
      relay.handle({
        'type': 'subscribe',
        'nonce': _nonce,
        'id': 1,
        'lastEventId': 's1:42',
      });
      await settle();
      expect(core.resumedFrom, ['s1:42']);
    },
  );

  test('unsubscribe cancels the subscription and runs nothing more', () async {
    relay.handle({'type': 'subscribe', 'nonce': _nonce, 'id': 1});
    await settle();
    ran.clear();
    expect(
      relay.handle({'type': 'unsubscribe', 'nonce': _nonce, 'id': 1}),
      isTrue,
    );
    await settle();
    expect(fake.subscribers, 0);
    fake.push('event: x\ndata: 1\n\n');
    await settle();
    expect(ran, isEmpty);
  });

  test('dispose cancels the subscriptions and the calls in flight, and '
      'answers nothing more', () async {
    final gate = Completer<CoreResponse>();
    fake.on('GET', '/api/slow', (r, p) => gate.future);
    relay
      ..handle({'type': 'subscribe', 'nonce': _nonce, 'id': 1})
      ..handle(call(2, 'GET', '/api/slow'));
    await settle();
    ran.clear();
    relay.dispose();
    expect(fake.subscribers, 0);
    expect(fake.cancelled.single.path, '/api/slow');
    gate.complete(FakeCore.json({}));
    fake.push('event: x\ndata: 1\n\n');
    await settle();
    expect(ran, isEmpty);
  });

  test('other types are not the relay\'s', () {
    for (final type in ['ready', 'history', 'barrier', 'ask']) {
      expect(relay.handle({'type': type, 'nonce': _nonce}), isFalse);
    }
  });

  test('a script the document can no longer run is dropped', () async {
    final failing = ChannelRelay(
      core,
      runJs: (js) => Future.error(StateError('no document')),
    );
    fake.on('GET', '/api/x', (r, p) => FakeCore.json({}));
    failing.handle(call(1, 'GET', '/api/x'));
    await settle();
    failing.dispose();
  });
}
