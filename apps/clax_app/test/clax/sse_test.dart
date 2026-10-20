import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/clax/sse.dart';
import 'package:clax_app/daemon/supervisor.dart' show DaemonEndpoint;
import 'package:flutter_test/flutter_test.dart';

import '../support/scripted_server.dart';

Future<List<SseMessage>> decode(String text, {String? lastEventId}) =>
    decodeSse(
      Stream.value(utf8.encode(text)),
      lastEventId: lastEventId,
    ).toList();

/// Writes [text] as one SSE response and keeps it open until [close]
/// completes (or closes it at once when [close] is null).
ScriptHandler sse(String text, {Future<void>? close}) => (_, res) async {
  res.statusCode = 200;
  res.headers.contentType = ContentType('text', 'event-stream');
  res.bufferOutput = false;
  res.write(text);
  await res.flush();
  if (close != null) await close;
  await res.close();
};

void main() {
  group('decodeSse', () {
    test('reads id, event and data blocks', () async {
      final ms = await decode(
        'id: 00000000000000aa-1\nevent: ready\ndata: {"resumed":false}\n\n'
        'id: 00000000000000aa-2\nevent: version\ndata: {"n":2}\n\n',
      );
      expect(ms.map((m) => (m.id, m.event, m.data)), [
        ('00000000000000aa-1', 'ready', '{"resumed":false}'),
        ('00000000000000aa-2', 'version', '{"n":2}'),
      ]);
    });

    test(
      'joins multi-line data, skips comments, and strips one leading space',
      () async {
        final ms = await decode(
          ': keep-alive\n\ndata:one\ndata:  two\n: inside\ndata\n\n',
        );
        expect(ms.single.event, 'message');
        expect(ms.single.data, 'one\n two\n');
      },
    );

    test('accepts CRLF and CR line ends', () async {
      final ms = await decode(
        'event: a\r\ndata: 1\r\n\r\nevent: b\rdata: 2\r\r',
      );
      expect(ms.map((m) => (m.event, m.data)), [('a', '1'), ('b', '2')]);
    });

    test('an ID holds for later messages until another replaces it', () async {
      final ms = await decode(
        'id: e-1\ndata: a\n\ndata: b\n\nid: e-2\ndata: c\n\n',
        lastEventId: 'e-0',
      );
      expect(ms.map((m) => m.id), ['e-1', 'e-1', 'e-2']);
      final seeded = await decode('data: x\n\n', lastEventId: 'e-0');
      expect(seeded.single.id, 'e-0');
    });

    test('a block without data, or cut off before its blank line, dispatches nothing', () async {
      final ms = await decode(
        'event: ping\nid: e-9\n\nevent: version\ndata: {"n":3}',
      );
      expect(ms, isEmpty);
    });
  });

  group('SseAborter', () {
    test(
      'an abort requested before the request exists is applied once it does',
      () {
        final a = SseAborter();
        a.abort();
        var aborted = 0;
        a.onAbort = () => aborted++;
        expect(aborted, 1);
        a.abort();
        expect(aborted, 1, reason: 'an abort applies once');
        expect(a.aborted, isTrue);
      },
    );

    test('an abort after the request exists calls it at once', () {
      final a = SseAborter();
      var aborted = 0;
      a.onAbort = () => aborted++;
      expect(aborted, 0);
      a.abort();
      expect(aborted, 1);
    });
  });

  group('events', () {
    late ScriptedServer server;
    late HttpClaxApi api;
    late List<Duration> delays;

    setUp(() async {
      server = await ScriptedServer.start();
      delays = [];
      api = HttpClaxApi.loopback(
        () => DaemonEndpoint(
          base: server.base,
          token: 'tok',
          pid: 1,
          port: server.base.port,
        ),
        eventRetryDelay: (d) async => delays.add(d),
      );
    });

    tearDown(() async {
      api.close();
      await server.close();
    });

    test(
      'opens /api/events with the token and filters, and parses events',
      () async {
        final hold = Completer<void>();
        server.handle(
          'GET',
          '/api/events',
          sse(
            'id: aa-1\nevent: ready\ndata: {"resumed":false}\n\n'
            ': keep-alive\n\n'
            'id: aa-2\nevent: version\ndata: {"artifact_id":"a1","n":2}\n\n',
            close: hold.future,
          ),
        );
        final got = await api
            .events(artifacts: ['a1', 'a2'], types: ['version', 'thread'])
            .take(2)
            .toList();
        hold.complete();
        expect(got.map((e) => (e.type, e.id)), [
          ('ready', 'aa-1'),
          ('version', 'aa-2'),
        ]);
        expect(got.first.data, {'resumed': false});
        expect(got.last.data, {'artifact_id': 'a1', 'n': 2});
        final req = server.requests.single;
        expect(req.query, {'artifact': 'a1,a2', 'types': 'version,thread'});
        expect(req.headers['authorization'], 'Bearer tok');
        expect(req.headers['accept'], 'text/event-stream');
        expect(req.headers.containsKey('last-event-id'), isFalse);
      },
    );

    test(
      'reconnects after the stream ends, sending the last event ID',
      () async {
        final hold = Completer<void>();
        server.handle(
          'GET',
          '/api/events',
          sse(
            'id: aa-1\nevent: ready\ndata: {"resumed":false}\n\nid: aa-2\nevent: version\ndata: {"n":2}\n\n',
          ),
        );
        server.handle(
          'GET',
          '/api/events',
          sse(
            'id: aa-3\nevent: ready\ndata: {"resumed":true}\n\nid: aa-4\nevent: version\ndata: {"n":3}\n\n',
            close: hold.future,
          ),
        );
        final got = await api.events().take(4).toList();
        hold.complete();
        expect(got.map((e) => e.id), ['aa-1', 'aa-2', 'aa-3', 'aa-4']);
        expect(got[2].data, {'resumed': true});
        expect(server.requests, hasLength(2));
        expect(server.requests[1].headers['last-event-id'], 'aa-2');
        expect(delays, [const Duration(milliseconds: 500)]);
      },
    );

    test(
      'retries a failing stream with a doubling wait, reset once it delivers',
      () async {
        final hold = Completer<void>();
        server.json('GET', '/api/events', 503, {
          'error': {'code': 'unavailable', 'message': 'x'},
        });
        server.json('GET', '/api/events', 503, {
          'error': {'code': 'unavailable', 'message': 'x'},
        });
        server.handle(
          'GET',
          '/api/events',
          sse('id: aa-1\nevent: ready\ndata: {}\n\n'),
        );
        server.handle(
          'GET',
          '/api/events',
          sse('id: aa-2\nevent: version\ndata: {}\n\n', close: hold.future),
        );
        final got = await api.events().take(2).toList();
        hold.complete();
        expect(got.map((e) => e.id), ['aa-1', 'aa-2']);
        expect(delays, const [
          Duration(milliseconds: 500),
          Duration(seconds: 1),
          Duration(milliseconds: 500),
        ]);
      },
    );

    test('a refused stream ends with an error and is not retried', () async {
      server.json('GET', '/api/events', 400, {
        'error': {'code': 'invalid_query', 'message': 'bad types'},
      });
      await expectLater(
        api.events().toList(),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.status, 'status', 400)
              .having((e) => e.code, 'code', 'stream_refused'),
        ),
      );
      expect(server.requests, hasLength(1));
      expect(delays, isEmpty);
    });

    test('a 401 is retried with the endpoint read again', () async {
      var token = 'old';
      final rotating = HttpClaxApi.loopback(
        () => DaemonEndpoint(
          base: server.base,
          token: token,
          pid: 1,
          port: server.base.port,
        ),
        eventRetryDelay: (d) async {
          delays.add(d);
          token = 'new';
        },
      );
      final hold = Completer<void>();
      server.json('GET', '/api/events', 401, {
        'error': {'code': 'unauthorized', 'message': 'token'},
      });
      server.handle(
        'GET',
        '/api/events',
        sse('id: aa-1\nevent: ready\ndata: {}\n\n', close: hold.future),
      );
      final e = await rotating.events().first;
      hold.complete();
      rotating.close();
      expect(e.type, 'ready');
      expect(server.requests.map((r) => r.headers['authorization']), [
        'Bearer old',
        'Bearer new',
      ]);
      expect(delays, [const Duration(milliseconds: 500)]);
    });

    test('data that is not a JSON object is carried raw', () async {
      final hold = Completer<void>();
      server.handle(
        'GET',
        '/api/events',
        sse('event: note\ndata: plain text\n\n', close: hold.future),
      );
      final e = await api.events().first;
      hold.complete();
      expect(
        e,
        isA<ClaxEvent>().having((e) => e.data, 'data', {'raw': 'plain text'}),
      );
    });

    test(
      'cancelling the subscription closes the connection and opens no other',
      () async {
        // A raw socket, so the client's close is seen as the end of its input.
        final raw = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
        var connections = 0;
        final closed = Completer<void>();
        raw.listen((sock) {
          connections++;
          sock.listen(
            (_) {},
            onDone: () {
              if (!closed.isCompleted) closed.complete();
              sock.destroy();
            },
            onError: (Object _) {},
          );
          sock.write(
            'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n'
            'id: aa-1\nevent: ready\ndata: {}\n\n',
          );
        });
        final rawApi = HttpClaxApi.loopback(
          () => DaemonEndpoint(
            base: Uri.parse('http://127.0.0.1:${raw.port}'),
            token: 'tok',
            pid: 1,
            port: raw.port,
          ),
          eventRetryDelay: (d) async => delays.add(d),
        );
        final first = await rawApi.events().first;
        expect(first.type, 'ready');
        await closed.future;
        expect(connections, 1);
        expect(delays, isEmpty);
        rawApi.close();
        await raw.close();
      },
    );

    test('cancelling between messages closes the connection at once', () async {
      // The daemon sends nothing more after the first message, so the
      // cancel comes while the decoder waits for the next line.
      final raw = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final closed = Completer<void>();
      raw.listen((sock) {
        sock.listen(
          (_) {},
          onDone: () {
            if (!closed.isCompleted) closed.complete();
            sock.destroy();
          },
          onError: (Object _) {},
        );
        sock.write(
          'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n'
          'id: aa-1\nevent: ready\ndata: {}\n\n: keep-alive\n\n',
        );
      });
      final rawApi = HttpClaxApi.loopback(
        () => DaemonEndpoint(
          base: Uri.parse('http://127.0.0.1:${raw.port}'),
          token: 'tok',
          pid: 1,
          port: raw.port,
        ),
        eventRetryDelay: (d) async => delays.add(d),
      );
      final got = Completer<void>();
      final sub = rawApi.events().listen((_) {
        if (!got.isCompleted) got.complete();
      });
      await got.future;
      await pumpEventQueue();
      await sub.cancel();
      await closed.future;
      expect(delays, isEmpty);
      rawApi.close();
      await raw.close();
    });

    test('an answer that does not come within the connect deadline is abandoned and retried', () async {
      final timers = ManualTimers();
      final raw = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final firstArrived = Completer<void>();
      final firstClosed = Completer<void>();
      var connections = 0;
      raw.listen((sock) {
        final n = ++connections;
        var text = '';
        sock.listen(
          (data) {
            text += utf8.decode(data);
            if (!text.contains('\r\n\r\n')) return;
            // The first connection never answers.
            if (n == 1 && !firstArrived.isCompleted) firstArrived.complete();
            if (n > 1 && !text.contains('HTTP/1.1 200')) {
              text += 'HTTP/1.1 200';
              sock.write(
                'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n'
                'id: aa-1\nevent: ready\ndata: {}\n\n',
              );
            }
          },
          onDone: () {
            if (n == 1 && !firstClosed.isCompleted) firstClosed.complete();
            sock.destroy();
          },
          onError: (Object _) {},
        );
      });
      final quick = HttpClaxApi.loopback(
        () => DaemonEndpoint(
          base: Uri.parse('http://127.0.0.1:${raw.port}'),
          token: 'tok',
          pid: 1,
          port: raw.port,
        ),
        eventRetryDelay: (d) async => delays.add(d),
        eventTimer: timers.call,
      );
      final first = quick.events().first;
      await firstArrived.future;
      timers.fire(HttpClaxApi.defaultEventConnectTimeout);
      await firstClosed.future;
      expect((await first).type, 'ready');
      expect(connections, 2);
      expect(delays, [const Duration(milliseconds: 500)]);
      quick.close();
      await raw.close();
    });

    test('a connection silent past the idle deadline is replaced, resuming from its last ID', () async {
      final timers = ManualTimers();
      final raw = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final requests = <String>[];
      raw.listen((sock) {
        final n = requests.length;
        requests.add('');
        var answered = false;
        sock.listen(
          (data) {
            requests[n] += utf8.decode(data);
            // Answer once the request's headers are in, then stay silent
            // with the socket left open.
            if (!answered && requests[n].contains('\r\n\r\n')) {
              answered = true;
              sock.write(
                'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n'
                '${n == 0 ? 'id: aa-1\nevent: ready\ndata: {}\n\n' : 'id: aa-2\nevent: version\ndata: {}\n\n'}',
              );
            }
          },
          onDone: sock.destroy,
          onError: (Object _) {},
        );
      });
      final quick = HttpClaxApi.loopback(
        () => DaemonEndpoint(
          base: Uri.parse('http://127.0.0.1:${raw.port}'),
          token: 'tok',
          pid: 1,
          port: raw.port,
        ),
        eventRetryDelay: (d) async => delays.add(d),
        eventTimer: timers.call,
      );
      final events = StreamIterator(quick.events());
      expect(await events.moveNext(), isTrue);
      expect(events.current.id, 'aa-1');
      expect(delays, isEmpty);
      timers.fire(HttpClaxApi.defaultEventIdleTimeout);
      expect(await events.moveNext(), isTrue);
      expect(events.current.id, 'aa-2');
      await events.cancel();
      expect(requests, hasLength(2));
      expect(requests[1].toLowerCase(), contains('last-event-id: aa-1'));
      expect(delays, [const Duration(milliseconds: 500)]);
      quick.close();
      await raw.close();
    });

    test(
      'a stream cancelled while its answer is on the way closes that answer',
      () async {
        // An endless event stream, as the daemon's.
        final raw = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
        final closed = Completer<void>();
        raw.listen((sock) {
          var answered = false;
          sock.listen(
            (data) {
              if (answered) return;
              answered = true;
              sock.write(
                'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n'
                ': keep-alive\n\n',
              );
            },
            onDone: () {
              if (!closed.isCompleted) closed.complete();
              sock.destroy();
            },
            onError: (Object _) {},
          );
        });
        final client = HttpClient();
        final answered = Completer<void>();
        final release = Completer<void>();
        final sub = reconnectingSse((_, _) async {
          final req = await client.getUrl(
            Uri.parse('http://127.0.0.1:${raw.port}/'),
          );
          final res = await req.close();
          answered.complete();
          await release.future;
          return res;
        }).listen((_) {});
        await answered.future;
        await sub.cancel();
        release.complete();
        await closed.future;
        client.close(force: true);
        await raw.close();
      },
    );

    test('the default deadlines allow for the daemon keep-alive', () {
      expect(
        HttpClaxApi.defaultEventConnectTimeout,
        const Duration(seconds: 10),
      );
      expect(
        HttpClaxApi.defaultEventIdleTimeout,
        const Duration(seconds: 45),
        reason: 'three 15 s keep-alives',
      );
    });
  });
}

/// Timers that fire only when the test says so.
class ManualTimer implements Timer {
  ManualTimer(this.duration, this._callback);

  final Duration duration;
  final void Function() _callback;
  bool _active = true;

  @override
  bool get isActive => _active;

  @override
  int get tick => 0;

  @override
  void cancel() => _active = false;

  void fire() {
    if (!_active) return;
    _active = false;
    _callback();
  }
}

class ManualTimers {
  final List<ManualTimer> all = [];

  Timer call(Duration d, void Function() callback) {
    final t = ManualTimer(d, callback);
    all.add(t);
    return t;
  }

  /// Fires the one active timer of [d].
  void fire(Duration d) {
    final live = [
      for (final t in all)
        if (t.isActive && t.duration == d) t,
    ];
    expect(live, hasLength(1), reason: 'active timers of $d');
    live.single.fire();
  }
}
