import 'dart:async';
import 'dart:convert';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/core/frame.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';
import '../support/scripted_core.dart';

final s1 = 'a' * 32;
final s2 = 'b' * 32;

String ready(
  String stream,
  int seq, {
  bool resumed = false,
  List<String> topics = const [],
}) =>
    'event: ready\ndata: ${jsonEncode({
      'stream': stream,
      'seq': seq,
      'resumed': resumed,
      'topics': topics,
      'caller': {'level': 'owner', 'viewer': null},
    })}\n\n';

String ev(String? id, String type, Map<String, Object?> data) =>
    '${id == null ? '' : 'id: $id\n'}event: $type\ndata: ${jsonEncode(data)}\n\n';

/// One subscription to the core's event stream: [script] writes its
/// blocks, waiting on whatever it likes between them, and the core ends
/// the stream (`stream_ended`) when it returns.
SubscriptionScript conn(
  Future<void> Function(Future<void> Function(String) write) script,
) => (s) => script((text) async => s.push(text));

void main() {
  late ScriptedCore server;
  late HttpClaxApi api;
  late ClaxStream stream;
  late Completer<void> hold;

  setUp(() {
    server = ScriptedCore();
    hold = Completer<void>();
    api = server.api(eventRetryDelay: (_) async {});
    stream = api.stream();
  });

  tearDown(() async {
    if (!hold.isCompleted) hold.complete();
    await stream.close();
    api.close();
  });

  /// Has each subscription to the core's event stream run [script], in
  /// order, the last one repeating.
  void onSubscribe(SubscriptionScript script) => server.core.script(script);

  /// Answers each `POST /api/stream/<id>` with the topics it subscribed.
  void acceptSubscribes(String id, {Completer<void>? posted}) =>
      server.handle('POST', '/api/stream/$id', (r) {
        final topics = (r.json! as Map)['subscribe'];
        // After the answer is made, as a server answers and then goes on.
        Future(() {
          if (posted != null && !posted.isCompleted) posted.complete();
        });
        return FakeCore.json({'seq': 50, 'topics': topics});
      });

  List<Object?> subscribeBodies(String id) => [
    for (final r in server.to('POST', '/api/stream/$id')) r.json,
  ];

  test(
    'subscribe waits for ready, then posts the topics as the owner',
    () async {
      onSubscribe(
        conn((write) async {
          await write(ready(s1, 41));
          await hold.future;
        }),
      );
      acceptSubscribes(s1);
      await stream.subscribe(['inbox', 'questions']);
      expect(server.core.resumes, [null], reason: 'one subscription, resuming nothing');
      final post = server.to('POST', '/api/stream/$s1').single;
      expect(post.as, CallAs.owner);
      expect(post.json, {
        'subscribe': ['inbox', 'questions'],
      });
    },
  );

  test(
    "events carry their topic and ID, and the core's resync passes through",
    () async {
      final subscribed = Completer<void>();
      onSubscribe(
        conn((write) async {
          await write(ready(s1, 41));
          await subscribed.future;
          await write(
            ev('$s1:43', 'inbox_item', {
              'topic': 'inbox',
              'item': {'id': 'i1'},
              'unread': 2,
            }),
          );
          await write(
            ev('$s1:44', 'question', {
              'topic': 'questions',
              'question': {'id': 'q1'},
            }),
          );
          await write(
            ev(null, 'resync', {'topic': 'inbox', 'reason': 'behind'}),
          );
          await hold.future;
        }),
      );
      acceptSubscribes(s1, posted: subscribed);
      final got = stream.events.take(3).toList();
      await stream.subscribe(['inbox', 'questions']);
      final es = await got;
      expect(es.map((e) => (e.topic, e.type, e.id)), [
        ('inbox', 'inbox_item', '$s1:43'),
        ('questions', 'question', '$s1:44'),
        ('inbox', 'resync', null),
      ]);
      expect(es[0].data['unread'], 2);
      expect(es[2].data['reason'], 'behind');
    },
  );

  test('a dropped connection resumes with the last event ID, without subscribing again', () async {
    final subscribed = Completer<void>();
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 41));
        await subscribed.future;
        await write(ev('$s1:43', 'inbox_item', {'topic': 'inbox'}));
      }),
    );
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 60, resumed: true, topics: ['inbox']));
        await write(ev('$s1:61', 'inbox_item', {'topic': 'inbox'}));
        await hold.future;
      }),
    );
    acceptSubscribes(s1, posted: subscribed);
    final got = stream.events.take(2).toList();
    await stream.subscribe(['inbox']);
    final es = await got;
    expect(es.map((e) => e.id), ['$s1:43', '$s1:61']);
    expect(es.map((e) => e.type), everyElement('inbox_item'));
    expect(server.core.resumes, hasLength(2));
    expect(server.core.resumes[1], '$s1:43');
    expect(subscribeBodies(s1), hasLength(1));
  });

  test("a drop before any event resumes from the ready's seq", () async {
    final subscribed = Completer<void>();
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 41));
        await subscribed.future;
      }),
    );
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 42, resumed: true, topics: ['inbox']));
        await write(ev('$s1:45', 'inbox_item', {'topic': 'inbox'}));
        await hold.future;
      }),
    );
    acceptSubscribes(s1, posted: subscribed);
    final got = stream.events.first;
    await stream.subscribe(['inbox']);
    expect((await got).id, '$s1:45');
    expect(
      server.core.resumes[1],
      '$s1:41',
    );
  });

  test('a new stream after a drop is subscribed again, and each topic gets a resync', () async {
    final subscribed = Completer<void>();
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 41));
        await subscribed.future;
      }),
    );
    onSubscribe(
      conn((write) async {
        await write(ready(s2, 7));
        await hold.future;
      }),
    );
    acceptSubscribes(s1, posted: subscribed);
    acceptSubscribes(s2);
    final got = stream.events.take(2).toList();
    await stream.subscribe(['inbox', 'questions']);
    final es = await got;
    expect(es.map((e) => (e.topic, e.type, e.data['reason'])), [
      ('inbox', 'resync', 'reconnect'),
      ('questions', 'resync', 'reconnect'),
    ]);
    expect(subscribeBodies(s2), [
      {
        'subscribe': ['inbox', 'questions'],
      },
    ]);
  });

  test('a topic the new stream refuses is dropped with unsubscribed; the rest resync', () async {
    final subscribed = Completer<void>();
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 41));
        await subscribed.future;
      }),
    );
    onSubscribe(
      conn((write) async {
        await write(ready(s2, 7));
        await hold.future;
      }),
    );
    acceptSubscribes(s1, posted: subscribed);
    final gone = {
      'error': {'code': 'not_found', 'message': 'no such artifact'},
    };
    server.json('POST', '/api/stream/$s2', 404, gone);
    server.json('POST', '/api/stream/$s2', 200, {
      'seq': 8,
      'topics': ['inbox'],
    });
    server.json('POST', '/api/stream/$s2', 404, gone);
    final got = stream.events.take(2).toList();
    await stream.subscribe(['inbox', 'artifact:7q3k9mzx2b4t']);
    final es = await got;
    expect(es.map((e) => (e.topic, e.type)), [
      ('artifact:7q3k9mzx2b4t', 'unsubscribed'),
      ('inbox', 'resync'),
    ]);
    expect(es[0].data['code'], 'not_found');
    expect(subscribeBodies(s2), [
      {
        'subscribe': ['inbox', 'artifact:7q3k9mzx2b4t'],
      },
      {
        'subscribe': ['inbox'],
      },
      {
        'subscribe': ['artifact:7q3k9mzx2b4t'],
      },
    ]);
  });

  /// A stream that drops once after its subscribe and comes back as [s2].
  void dropToNewStream() {
    final subscribed = Completer<void>();
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 41));
        await subscribed.future;
      }),
    );
    onSubscribe(
      conn((write) async {
        await write(ready(s2, 7));
        await hold.future;
      }),
    );
    acceptSubscribes(s1, posted: subscribed);
  }

  final unavailable = {
    'error': {'code': 'unavailable', 'message': 'busy'},
  };

  test(
    'a re-subscribe that fails for a while is retried until it holds',
    () async {
      dropToNewStream();
      server.json('POST', '/api/stream/$s2', 503, unavailable);
      server.json('POST', '/api/stream/$s2', 503, unavailable);
      acceptSubscribes(s2);
      final got = stream.events.first;
      await stream.subscribe(['inbox']);
      final e = await got;
      expect(
        (e.topic, e.type, e.data['reason']),
        ('inbox', 'resync', 'reconnect'),
      );
      expect(subscribeBodies(s2), hasLength(3));
    },
  );

  test(
    'a topic whose single re-subscribe fails for a while is retried alone',
    () async {
      dropToNewStream();
      final gone = {
        'error': {'code': 'not_found', 'message': 'no such artifact'},
      };
      server.json('POST', '/api/stream/$s2', 404, gone);
      server.json('POST', '/api/stream/$s2', 503, unavailable);
      server.json('POST', '/api/stream/$s2', 404, gone);
      acceptSubscribes(s2);
      final got = stream.events.take(2).toList();
      await stream.subscribe(['inbox', 'artifact:7q3k9mzx2b4t']);
      final es = await got;
      expect(es.map((e) => (e.topic, e.type)), [
        ('artifact:7q3k9mzx2b4t', 'unsubscribed'),
        ('inbox', 'resync'),
      ]);
      expect(subscribeBodies(s2).last, {
        'subscribe': ['inbox'],
      });
    },
  );

  test(
    'a re-subscribe that keeps failing gives up with unsubscribed per topic',
    () async {
      dropToNewStream();
      server.json('POST', '/api/stream/$s2', 503, unavailable);
      final got = stream.events.take(2).toList();
      await stream.subscribe(['inbox', 'questions']);
      final es = await got;
      expect(es.map((e) => (e.topic, e.type, e.data['code'])), [
        ('inbox', 'unsubscribed', 'unavailable'),
        ('questions', 'unsubscribed', 'unavailable'),
      ]);
      expect(subscribeBodies(s2), hasLength(SseClaxStream.restoreAttempts));
    },
  );

  test(
    'a refused subscribe throws, and the topic is not subscribed again later',
    () async {
      final refused = Completer<void>();
      onSubscribe(
        conn((write) async {
          await write(ready(s1, 41));
          await refused.future;
        }),
      );
      onSubscribe(
        conn((write) async {
          await write(ready(s2, 7));
          await write(ev('$s2:9', 'question', {'topic': 'questions'}));
          await hold.future;
        }),
      );
      server.json('POST', '/api/stream/$s1', 403, {
        'error': {'code': 'forbidden', 'message': 'owner only'},
      });
      acceptSubscribes(s2);
      final first = stream.events.first;
      await expectLater(
        stream.subscribe(['inbox']),
        throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'forbidden')),
      );
      refused.complete();
      // The event shows the new stream is up and its ready was handled.
      expect((await first).id, '$s2:9');
      await stream.subscribe(['questions']);
      expect(subscribeBodies(s2), [
        {
          'subscribe': ['questions'],
        },
      ]);
    },
  );

  test(
    'a subscribe to a stream the core no longer holds goes to the next one',
    () async {
      final posted = Completer<void>();
      onSubscribe(
        conn((write) async {
          await write(ready(s1, 41));
          await posted.future;
        }),
      );
      onSubscribe(
        conn((write) async {
          await write(ready(s2, 7));
          await hold.future;
        }),
      );
      server.handle('POST', '/api/stream/$s1', (_) {
        Future(posted.complete);
        return FakeCore.error(404, 'unknown_stream', 'no such stream');
      });
      acceptSubscribes(s2);
      await stream.subscribe(['inbox']);
      expect(
        subscribeBodies(s2),
        everyElement({
          'subscribe': ['inbox'],
        }),
      );
      expect(subscribeBodies(s2), isNotEmpty);
    },
  );

  test('a stream refused at open ends its events with an error and fails subscribe', () async {
    onSubscribe((s) => s.end('stream_refused', status: 403));
    final events = stream.events.toList();
    await expectLater(stream.subscribe(['inbox']), throwsStateError);
    await expectLater(
      events,
      throwsA(
        isA<ClaxApiError>()
            .having((e) => e.status, 'status', 403)
            .having((e) => e.code, 'code', 'stream_refused'),
      ),
    );
    expect(server.core.resumes, hasLength(1));
  });

  test('close ends the events, and a later subscribe throws', () async {
    onSubscribe(
      conn((write) async {
        await write(ready(s1, 41));
        await hold.future;
      }),
    );
    acceptSubscribes(s1);
    final done = stream.events.toList();
    await stream.subscribe(['inbox']);
    await stream.close();
    expect(await done, isEmpty);
    await expectLater(stream.subscribe(['questions']), throwsStateError);
  });
}
