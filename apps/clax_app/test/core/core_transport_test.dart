import 'dart:async';
import 'dart:convert';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/clax/sse.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/frame.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';

String ev(String? id, String type, Map<String, Object?> data) =>
    '${id == null ? '' : 'id: $id\n'}event: $type\ndata: ${jsonEncode(data)}\n\n';

/// A [CoreCalls] whose every call goes unanswered with [code].
class _Unanswering implements CoreCalls {
  _Unanswering(this.code);
  final String code;

  @override
  CoreCall call(CoreRequest r) =>
      CoreCall(0, Future.value(CoreUnanswered(code, 'gone')));

  @override
  void cancel(int tag) {}

  @override
  CoreSubscription subscribe({String? lastEventId}) =>
      throw UnimplementedError();
}

/// A [CoreCalls] whose calls the test answers, whenever it likes, and
/// which cancels nothing.
class _Held implements CoreCalls {
  final List<Completer<CoreAnswer>> answers = [];
  final List<int> cancels = [];

  @override
  CoreCall call(CoreRequest r) {
    answers.add(Completer());
    return CoreCall(answers.length - 1, answers.last.future);
  }

  @override
  void cancel(int tag) => cancels.add(tag);

  @override
  CoreSubscription subscribe({String? lastEventId}) =>
      throw UnimplementedError();
}

/// A timer that never fires by itself: the test runs its callback.
class _HeldTimer implements Timer {
  bool _active = true;

  @override
  bool get isActive => _active;

  @override
  int get tick => 0;

  @override
  void cancel() => _active = false;
}

void main() {
  group('HttpClaxApi over the core', () {
    late FakeClaxApi fake;
    late FakeCore core;
    late HttpClaxApi api;

    setUp(() {
      fake = FakeClaxApi();
      core = FakeCore.fromApi(fake);
      api = HttpClaxApi.over(CoreTransport(core));
    });

    /// The requests the core saw for [method] [path] (without the query).
    List<CoreRequest> to(String method, String path) => [
      for (final r in core.requests)
        if (r.method == method && Uri.parse(r.path).path == path) r,
    ];

    String? header(CoreRequest r, String name) {
      for (final (k, v) in r.headers) {
        if (k.toLowerCase() == name) return v;
      }
      return null;
    }

    test('every call speaks for the owner and carries no token', () async {
      final s = await api.registerAppSession(installId: 'install-1', pid: 7);
      final created = await api.createArtifact(
        PublishRequest(title: 'Tea', files: {'index.html': '<p>tea'}),
      );
      final aid = created.artifact.id;
      expect((await api.listArtifacts()).map((a) => a.title), ['Tea']);

      final item = fake.sendComment(aid, 'bigger');
      final thread = await api.postOwnerComment(aid, item.threadId, 'and blue');
      expect(thread.comments.map((c) => c.body), ['bigger', 'and blue']);

      await api.batchDocs(aid, const [
        DocWrite(op: 'set', collection: 'notes', docId: 'n1', data: {'t': 1}),
      ]);
      expect((await fake.getDoc(aid, 'notes/n1'))!.data, {'t': 1});

      expect(core.requests, isNotEmpty);
      for (final r in core.requests) {
        expect(r.as, CallAs.owner, reason: '$r');
        expect(header(r, 'authorization'), isNull, reason: '$r');
      }
      // The create is an agent call; the others are the owner's own.
      final create = to('POST', '/api/artifacts').single;
      expect(header(create, 'x-clax-session'), s.id);
      expect(header(create, 'content-type'), startsWith('application/json'));
      for (final r in [
        ...to('GET', '/api/artifacts'),
        ...to('POST', '/api/artifacts/$aid/threads/${item.threadId}/comments'),
        ...to('POST', '/api/artifacts/$aid/docs:batch'),
      ]) {
        expect(header(r, 'x-clax-session'), isNull, reason: '$r');
      }
    });

    test('a feedback poll is answered when a comment comes', () async {
      final s = await api.registerAppSession(installId: 'install-1', pid: 7);
      final aid = (await api.createArtifact(
        PublishRequest(title: 'Tea', files: {'index.html': '<p>tea'}),
      )).artifact.id;
      var answered = false;
      final poll = api
          .pollFeedback(s.id, wait: const Duration(seconds: 20))
          .whenComplete(() => answered = true);
      await pumpEventQueue();
      expect(answered, isFalse);
      fake.sendComment(aid, 'bigger');
      expect((await poll).items.single.body, 'bigger');
      final r = to('GET', '/api/sessions/${s.id}/feedback').single;
      expect(Uri.parse(r.path).queryParameters, {'tier': 'wait', 'wait': '20'});
    });

    test('close cancels the calls in flight and ends the streams', () async {
      final hanging = FakeCore()
        ..on('GET', '/api/artifacts', (r, p) => Completer<CoreResponse>().future)
        ..on(
          'GET',
          '/api/sessions/{sid}/feedback',
          (r, p) => Completer<CoreResponse>().future,
        );
      final transport = CoreTransport(hanging);
      final closing = HttpClaxApi.over(transport);
      final failures = <Object>[];
      final calls = [
        closing.listArtifacts().then<void>((_) {}, onError: failures.add),
        closing
            .pollFeedback('s1', wait: const Duration(seconds: 20))
            .then<void>((_) {}, onError: failures.add),
      ];
      var ended = false;
      transport.sse().listen((_) {}, onDone: () => ended = true);
      await pumpEventQueue();
      expect(hanging.subscribers, 1);

      closing.close();
      await Future.wait(calls);
      await pumpEventQueue();

      expect(
        hanging.cancelled.map((r) => Uri.parse(r.path).path),
        unorderedEquals(['/api/artifacts', '/api/sessions/s1/feedback']),
        reason: 'each call in flight is cancelled in the core',
      );
      final closed = isA<ClaxApiError>()
          .having((e) => e.status, 'status', 0)
          .having((e) => e.code, 'code', ClaxApiError.unreachable)
          .having((e) => e.unanswered, 'unanswered', isTrue)
          .having((e) => e.body['reason'], 'reason', 'closed');
      expect(failures, [closed, closed]);
      expect(ended, isTrue, reason: 'the stream ends');
      expect(hanging.subscribers, 0, reason: 'its subscription is cancelled');

      // Nothing goes through it afterwards.
      await expectLater(closing.listArtifacts(), throwsA(closed));
      var after = false;
      transport.sse().listen((_) {}, onDone: () => after = true);
      await pumpEventQueue();
      expect(after, isTrue);
      expect(hanging.subscriptions, 1);
      expect(hanging.requests.map((r) => Uri.parse(r.path).path), [
        '/api/artifacts',
        '/api/sessions/s1/feedback',
      ]);
    });

    test("a refusal throws the core's error", () async {
      await api.registerAppSession(installId: 'install-1', pid: 7);
      final aid = (await api.createArtifact(
        PublishRequest(title: 'Tea', files: {'index.html': '<p>tea'}),
      )).artifact.id;
      await expectLater(
        api.publish(aid, PublishRequest(ifVersion: 9, files: {'index.html': 'x'})),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.status, 'status', 409)
              .having((e) => e.code, 'code', 'conflict'),
        ),
      );
    });

    test('a call the core did not answer is unreachable, its reason in the body', () async {
      for (final reason in ['core_closed', 'core_failed', 'frame_refused', 'malformed']) {
        final dead = HttpClaxApi.over(CoreTransport(_Unanswering(reason)));
        await expectLater(
          dead.listArtifacts(),
          throwsA(
            isA<ClaxApiError>()
                .having((e) => e.status, 'status', 0)
                .having((e) => e.unanswered, 'unanswered', isTrue)
                .having((e) => e.code, 'code', ClaxApiError.unreachable)
                .having((e) => e.body['reason'], 'reason', reason)
                .having((e) => e.message, 'message', contains(reason)),
          ),
        );
      }
    });

    test('a call with no answer within its limit is status 0 timeout', () async {
      final timers = <(Duration, void Function())>[];
      final hanging = FakeCore()
        ..on('GET', '/api/artifacts', (r, p) => Completer<CoreResponse>().future);
      final slow = HttpClaxApi.over(
        CoreTransport(
          hanging,
          timer: (d, f) {
            timers.add((d, f));
            return _HeldTimer();
          },
        ),
        requestTimeout: const Duration(seconds: 35),
      );
      Object? failure;
      final call = slow.listArtifacts().then<void>(
        (_) {},
        onError: (Object e) => failure = e,
      );
      await pumpEventQueue();
      expect(failure, isNull);
      expect(timers.single.$1, const Duration(seconds: 35));
      timers.single.$2();
      await call;
      expect(
        failure,
        isA<ClaxApiError>()
            .having((e) => e.status, 'status', 0)
            .having((e) => e.code, 'code', ClaxApiError.timeout)
            .having((e) => e.unanswered, 'unanswered', isTrue),
      );
    });

    test('a call with no answer within its limit is cancelled in the core', () async {
      final timers = <(Duration, void Function())>[];
      final hanging = FakeCore()
        ..on(
          'GET',
          '/api/sessions/{sid}/feedback',
          (r, p) => Completer<CoreResponse>().future,
        );
      final slow = HttpClaxApi.over(
        CoreTransport(
          hanging,
          timer: (d, f) {
            timers.add((d, f));
            return _HeldTimer();
          },
        ),
      );
      Object? failure;
      final poll = slow
          .pollFeedback('s1', wait: const Duration(seconds: 20))
          .then<void>((_) {}, onError: (Object e) => failure = e);
      await pumpEventQueue();
      expect(hanging.cancelled, isEmpty);
      timers.single.$2();
      await poll;
      expect(failure, isA<ClaxApiError>().having((e) => e.code, 'code', ClaxApiError.timeout));
      expect(
        hanging.cancelled.map((r) => Uri.parse(r.path).path),
        ['/api/sessions/s1/feedback'],
      );
    });

    test('an answer after the time limit is dropped, and one in time stops the deadline', () async {
      final timers = <(_HeldTimer, void Function())>[];
      final core = _Held();
      final api = HttpClaxApi.over(
        CoreTransport(
          core,
          timer: (d, f) {
            final t = _HeldTimer();
            timers.add((t, f));
            return t;
          },
        ),
      );
      Object? failure;
      final late = api.listArtifacts().then<void>(
        (_) {},
        onError: (Object e) => failure = e,
      );
      await pumpEventQueue();
      timers.single.$2();
      await late;
      expect(failure, isA<ClaxApiError>().having((e) => e.code, 'code', ClaxApiError.timeout));
      expect(core.cancels, [0]);
      // The core's answer comes after all: nothing hears it, and nothing
      // fails.
      core.answers.single.complete(
        CoreAnswered(FakeCore.json({'artifacts': <Object?>[]})),
      );
      await pumpEventQueue();

      final inTime = api.listArtifacts();
      await pumpEventQueue();
      core.answers.last.complete(
        CoreAnswered(FakeCore.json({'artifacts': <Object?>[]})),
      );
      expect(await inTime, isEmpty);
      expect(timers.last.$1.isActive, isFalse, reason: 'the deadline was cancelled');
      expect(core.cancels, [0], reason: 'an answered call is not cancelled');
    });

    test('a feedback poll is an owner call on its session, given wait + pollSlack', () async {
      final limits = <Duration>[];
      final timed = HttpClaxApi.over(
        CoreTransport(
          core,
          timer: (d, f) {
            limits.add(d);
            return _HeldTimer();
          },
        ),
        pollSlack: const Duration(seconds: 20),
      );
      final s = await timed.registerAppSession(installId: 'install-1', pid: 7);
      final aid = (await timed.createArtifact(
        PublishRequest(title: 'Tea', files: {'index.html': '<p>tea'}),
      )).artifact.id;
      final poll = timed.pollFeedback(s.id, wait: const Duration(seconds: 50));
      await pumpEventQueue();
      expect(limits.last, const Duration(seconds: 70));
      final r = to('GET', '/api/sessions/${s.id}/feedback').single;
      expect(Uri.parse(r.path).queryParameters, {'tier': 'wait', 'wait': '50'});
      expect(r.as, CallAs.owner);
      expect(header(r, 'x-clax-session'), isNull, reason: 'the path names the session');
      expect(header(r, 'authorization'), isNull);
      fake.sendComment(aid, 'bigger');
      expect((await poll).items.single.body, 'bigger');
    });

    test('across a stream the core ended, the topics are subscribed again and resynced', () async {
      const aid = 'abcdefghjkmn';
      final again = HttpClaxApi.over(
        CoreTransport(core, retryDelay: (_) async {}),
        eventRetryDelay: (_) async {},
      );
      final stream = again.stream();
      final events = <StreamEvent>[];
      final sub = stream.events.listen(events.add);
      await stream.subscribe(['artifact:$aid']);
      expect(to('POST', '/api/stream/fake-1'), hasLength(1));
      core.end('stream_ended');
      await pumpEventQueue();
      // The core gives a new stream, not the old one resumed.
      expect(core.subscriptions, 2);
      expect(FakeCore.bodyOf(to('POST', '/api/stream/fake-2').single), {
        'subscribe': ['artifact:$aid'],
      });
      expect(events.single.type, 'resync');
      expect(events.single.topic, 'artifact:$aid');
      expect(events.single.data['reason'], 'reconnect');
      core.push(ev('fake-2:1', 'version', {'topic': 'artifact:$aid', 'n': 3}));
      await pumpEventQueue();
      expect(events.last.type, 'version');
      expect(events.last.id, 'fake-2:1');
      await sub.cancel();
      await stream.close();
    });

    test("the stream's events end with an error when the core closes", () async {
      final stream = api.stream();
      final errors = <Object>[];
      var done = false;
      stream.events.listen(
        (_) {},
        onError: errors.add,
        onDone: () => done = true,
      );
      await stream.subscribe(['inbox']);
      core.close();
      await pumpEventQueue();
      expect(
        errors.single,
        isA<ClaxApiError>()
            .having((e) => e.code, 'code', ClaxApiError.unreachable)
            .having((e) => e.body['reason'], 'reason', 'core_closed'),
      );
      expect(done, isTrue);
      expect(() => stream.subscribe(['questions']), throwsStateError);
    });

    test("the stream delivers a pushed version to its artifact's topic", () async {
      const aid = 'abcdefghjkmn';
      final stream = api.stream();
      final events = <StreamEvent>[];
      final sub = stream.events.listen(events.add);
      await stream.subscribe(['artifact:$aid']);
      final posted = to('POST', '/api/stream/fake-1').single;
      expect(FakeCore.bodyOf(posted), {
        'subscribe': ['artifact:$aid'],
      });
      core.push(ev('fake-1:1', 'version', {'topic': 'artifact:$aid', 'n': 2}));
      await pumpEventQueue();
      expect(events.single.topic, 'artifact:$aid');
      expect(events.single.type, 'version');
      expect(events.single.data['n'], 2);
      expect(events.single.id, 'fake-1:1');
      await sub.cancel();
      await stream.close();
      expect(core.subscribers, 0);
    });
  });

  group('CoreTransport.sse', () {
    test('parses the blocks and passes on ready and events', () async {
      final core = FakeCore();
      final messages = <SseMessage>[];
      final sub = CoreTransport(core).sse().listen(messages.add);
      await pumpEventQueue();
      core.push(ev('fake-1:1', 'version', {'n': 1}));
      core.push(': keep-alive\n\n');
      await pumpEventQueue();
      expect(messages.map((m) => m.event), ['ready', 'version']);
      expect(messages.last.id, 'fake-1:1');
      expect(jsonDecode(messages.last.data), {'n': 1});
      await sub.cancel();
      expect(core.subscribers, 0, reason: 'a cancel ends the subscription');
    });

    test('a stream the core ended is subscribed again from where it stopped', () async {
      final core = FakeCore();
      final delays = <Duration>[];
      final messages = <SseMessage>[];
      final sub = CoreTransport(core, retryDelay: (d) async => delays.add(d))
          .sse()
          .listen(messages.add);
      await pumpEventQueue();
      core.push(ev('fake-1:4', 'version', {'n': 1}));
      core.end('stream_ended');
      await pumpEventQueue();
      expect(core.resumes, [null, 'fake-1:4']);
      expect(core.subscribers, 1);
      expect(messages.map((m) => m.event), ['ready', 'version', 'ready']);
      expect(messages.any((m) => m.event == 'end'), isFalse);
      // One that ends before any event waits longer each time.
      core.end('stream_ended');
      await pumpEventQueue();
      core.end('stream_ended');
      await pumpEventQueue();
      expect(delays, const [
        Duration(milliseconds: 500),
        Duration(seconds: 1),
        Duration(seconds: 2),
      ]);
      await sub.cancel();
    });

    test('the wait before subscribing again goes back to 0.5 s after a stream that carried an event', () async {
      final core = FakeCore();
      final delays = <Duration>[];
      final sub = CoreTransport(core, retryDelay: (d) async => delays.add(d))
          .sse()
          .listen((_) {});
      await pumpEventQueue();
      core.end('stream_ended');
      await pumpEventQueue();
      core.end('stream_ended');
      await pumpEventQueue();
      core.push(ev('fake-3:1', 'version', {}));
      core.end('stream_ended');
      await pumpEventQueue();
      expect(delays, const [
        Duration(milliseconds: 500),
        Duration(seconds: 1),
        Duration(milliseconds: 500),
      ]);
      expect(core.subscribers, 1);
      await sub.cancel();
    });

    test('a stream that panicked in the core (internal) is subscribed again', () async {
      final core = FakeCore();
      final delays = <Duration>[];
      final sub = CoreTransport(core, retryDelay: (d) async => delays.add(d))
          .sse()
          .listen((_) {});
      await pumpEventQueue();
      core.push(ev('fake-1:4', 'version', {}));
      core.end('internal');
      await pumpEventQueue();
      expect(core.resumes, [null, 'fake-1:4']);
      expect(core.subscribers, 1);
      expect(delays, const [Duration(milliseconds: 500)]);
      await sub.cancel();
    });

    test('a cancel while the stream waits to subscribe again opens no subscription', () async {
      final core = FakeCore();
      final wait = Completer<void>();
      final sub = CoreTransport(core, retryDelay: (_) => wait.future)
          .sse()
          .listen((_) {});
      await pumpEventQueue();
      core.end('stream_ended');
      await pumpEventQueue();
      expect(core.subscribers, 0);
      await sub.cancel();
      wait.complete();
      await pumpEventQueue();
      expect(core.subscriptions, 1);
      expect(core.subscribers, 0);
    });

    test('the resume point asked for wins over the last ID seen', () async {
      final core = FakeCore();
      final sub = CoreTransport(core, retryDelay: (_) async {})
          .sse(resumeId: () => 's:9')
          .listen((_) {});
      await pumpEventQueue();
      core.push(ev('fake-1:4', 'version', {}));
      core.end('stream_ended');
      await pumpEventQueue();
      expect(core.resumes, ['s:9', 's:9']);
      await sub.cancel();
    });

    for (final reason in ['core_closed', 'core_failed', 'malformed']) {
      test('a stream that ends $reason ends unreachable, the reason in the body', () async {
        final core = FakeCore();
        final errors = <Object>[];
        var done = false;
        CoreTransport(core, retryDelay: (_) async {}).sse().listen(
          (_) {},
          onError: errors.add,
          onDone: () => done = true,
        );
        await pumpEventQueue();
        core.end(reason);
        await pumpEventQueue();
        expect(
          errors.single,
          isA<ClaxApiError>()
              .having((e) => e.status, 'status', 0)
              .having((e) => e.code, 'code', ClaxApiError.unreachable)
              .having((e) => e.body['reason'], 'reason', reason),
        );
        expect(done, isTrue);
        expect(core.subscribers, 0);
      });
    }

    for (final status in [400, 403, 404]) {
      test('a stream the core refuses $status ends with stream_refused and that status', () async {
        final core = FakeCore();
        final errors = <Object>[];
        var done = false;
        CoreTransport(core, retryDelay: (_) async {}).sse().listen(
          (_) {},
          onError: errors.add,
          onDone: () => done = true,
        );
        await pumpEventQueue();
        core.end('stream_refused', status: status);
        await pumpEventQueue();
        expect(
          errors.single,
          isA<ClaxApiError>()
              .having((e) => e.status, 'status', status)
              .having((e) => e.code, 'code', 'stream_refused'),
        );
        expect(done, isTrue);
        expect(core.subscribers, 0);
      });
    }

    test('a stream the core refuses otherwise is subscribed again after the backoff', () async {
      final core = FakeCore();
      final delays = <Duration>[];
      final errors = <Object>[];
      final messages = <SseMessage>[];
      final sub = CoreTransport(core, retryDelay: (d) async => delays.add(d))
          .sse()
          .listen(messages.add, onError: errors.add);
      await pumpEventQueue();
      core.end('stream_refused', status: 503);
      await pumpEventQueue();
      core.end('stream_refused', status: 500);
      await pumpEventQueue();
      expect(errors, isEmpty);
      expect(core.subscribers, 1);
      expect(delays, const [Duration(milliseconds: 500), Duration(seconds: 1)]);
      expect(messages.map((m) => m.event), ['ready', 'ready', 'ready']);
      await sub.cancel();
    });
  });
}
