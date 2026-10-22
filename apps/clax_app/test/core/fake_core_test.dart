import 'dart:convert';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/frame.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';

/// The JSON body of [a]'s response; fails the test when it went
/// unanswered.
Map<String, Object?> jsonOf(CoreAnswer a) =>
    (jsonDecode(utf8.decode(statusOf(a).$2.body)) as Map).cast();

/// [a]'s status and response; fails the test when it went unanswered.
(int, CoreResponse) statusOf(CoreAnswer a) => switch (a) {
  CoreAnswered(:final response) => (response.status, response),
  CoreUnanswered() => throw TestFailure('expected an answer, got $a'),
};

void main() {
  test('dispatches on method and path, and records every request', () async {
    final core = FakeCore()
      ..on('GET', '/api/artifacts/{aid}', (r, p) => FakeCore.json({'id': p['aid']}));
    final r = await core.call(const CoreRequest('GET', '/api/artifacts/a%2Fb?x=1')).answer;
    expect(jsonOf(r), {'id': 'a/b'});
    expect(core.requests.map((r) => r.path), ['/api/artifacts/a%2Fb?x=1']);
  });

  test('refuses what the core refuses: 404 not_in_app', () async {
    final core = FakeCore()
      ..on('GET', '/api/artifacts', (r, p) => FakeCore.json({}))
      ..on('GET', '/a/{aid}', (r, p) => FakeCore.json({}))
      ..on('GET', '/api/stream', (r, p) => FakeCore.json({}));
    const id = '7q3k9mzx2b4t';
    for (final (r, admitted) in [
      (const CoreRequest('GET', '/api/artifacts'), true),
      (const CoreRequest('GET', '/api/artifacts', as: CallAs.content, host: id), true),
      (const CoreRequest('GET', '/a/$id?app=1', as: CallAs.shellPage), true),
      // A route that holds its connection open answers no call.
      (const CoreRequest('GET', '/api/stream'), false),
      (const CoreRequest('GET', '/api/stream?x=1'), false),
      (const CoreRequest('GET', '/api/events'), false),
      (const CoreRequest('GET', '/api/artifacts/$id/room'), false),
      (const CoreRequest('GET', '/api/artifacts/$id/live'), false),
      (const CoreRequest('POST', '/mcp'), false),
      (const CoreRequest('POST', '/api/admin/shutdown'), false),
      // A page reaches only its own paths.
      (const CoreRequest('GET', '/api/artifacts', as: CallAs.shellPage), false),
      (const CoreRequest('POST', '/a/$id', as: CallAs.shellPage), false),
      (const CoreRequest('GET', '/assets/x.js', as: CallAs.shellPage), false),
      (const CoreRequest('HEAD', '/assets/x.js', as: CallAs.shellPage), false),
      (const CoreRequest('GET', '/favicon.ico', as: CallAs.shellPage), false),
      (const CoreRequest('GET', '/api/artifacts', as: CallAs.content), false),
      (const CoreRequest('PUT', '/api/artifacts', as: CallAs.content, host: id), false),
      (
        const CoreRequest('GET', '/api/artifacts', as: CallAs.content, host: '7Q3K9MZX2B4T'),
        false,
      ),
    ]) {
      final (status, res) = statusOf(await core.call(r).answer);
      if (admitted) {
        expect(status, 200, reason: '$r');
      } else {
        expect(status, 404, reason: '$r');
        expect((jsonOf(CoreAnswered(res))['error']! as Map)['code'], 'not_in_app', reason: '$r');
      }
    }
  });

  test('a call no route answers fails the test', () async {
    final unrouted = <Object>[];
    final core = FakeCore(unrouted: unrouted.add);
    await expectLater(
      core.call(const CoreRequest('POST', '/api/artifacts/x')).answer,
      throwsA(isA<TestFailure>()),
    );
    expect(unrouted.single.toString(), contains('POST /api/artifacts/x'));
  });

  test("a subscription starts with the hub's ready block", () async {
    final core = FakeCore();
    final first = await core.subscribe().blocks.first;
    expect(first, startsWith('event: ready\n'));
    final data = jsonDecode(first.split('data: ')[1].trim()) as Map;
    expect(data['resumed'], false);
    expect(data['stream'], isA<String>());
  });

  test('a closed core answers no call and ends every subscription', () async {
    final core = FakeCore()..on('GET', '/api/a', (r, p) => FakeCore.json({}));
    final live = core.subscribe();
    core.close();
    expect(await live.blocks.last, endBlock('core_closed'));
    final r = await core.call(const CoreRequest('GET', '/api/a')).answer;
    expect((r as CoreUnanswered).code, 'core_closed');
    expect(core.requests, isEmpty, reason: 'a closed core is not called');
    expect(await core.subscribe().blocks.toList(), [endBlock('core_closed')]);
  });

  test('pushes blocks to live subscriptions and ends them', () async {
    final core = FakeCore();
    final a = core.subscribe();
    // A resume point is ignored, as CoreClient ignores it.
    final b = core.subscribe(lastEventId: '7');
    b.cancel();
    core.push('event: version\n\n');
    core.end('stream_ended');
    final blocks = await a.blocks.toList();
    expect(blocks.skip(1), ['event: version\n\n', endBlock('stream_ended')]);
    expect(core.subscribers, 0);
  });

  test("fromApi answers in the daemon's shapes", () async {
    final api = FakeClaxApi();
    final core = FakeCore.fromApi(api);
    final s = await core.call(
      CoreRequest(
        'POST',
        '/api/sessions',
        body: utf8.encode(jsonEncode({
          'harness': 'app',
          'harness_session_id': 'install-1',
          'cwd': '',
          'pid': 1,
        })),
      ),
    ).answer;
    final sid = Session.fromJson((jsonOf(s)['session']! as Map).cast()).id;

    final created = await core.call(
      CoreRequest(
        'POST',
        '/api/artifacts',
        body: utf8.encode(
          jsonEncode(
            PublishRequest(
              title: 'Tea',
              entries: {
                'index.html': FileEntry.text('<p>tea'),
                'a.bin': FileEntry.bytes(utf8.encode('xy'), type: 'application/x'),
              },
            ).toJson(),
          ),
        ),
      ),
    ).answer;
    final published = PublishResult.fromJson(jsonOf(created));
    final aid = published.artifact.id;
    expect(published.version.files['a.bin']!.contentType, 'application/x');
    expect(await api.readFileBytes(aid, 1, 'a.bin'), utf8.encode('xy'));

    final list = await core.call(const CoreRequest('GET', '/api/artifacts')).answer;
    expect(
      [for (final a in jsonOf(list)['artifacts']! as List) ArtifactSummary.fromJson((a as Map).cast()).title],
      ['Tea'],
    );

    final item = api.sendComment(aid, 'bigger');
    final polled = await core.call(
      CoreRequest('GET', '/api/sessions/$sid/feedback?tier=wait&wait=20'),
    ).answer;
    expect(FeedbackBatch.fromJson(jsonOf(polled)).items.single.body, 'bigger');

    final commented = await core.call(
      CoreRequest(
        'POST',
        '/api/artifacts/$aid/threads/${item.threadId}/comments',
        body: utf8.encode('{"body":"and blue"}'),
      ),
    ).answer;
    final thread = ThreadView.fromJson((jsonOf(commented)['thread']! as Map).cast());
    expect(thread.comments.map((c) => c.body), ['bigger', 'and blue']);

    final batch = await core.call(
      CoreRequest(
        'POST',
        '/api/artifacts/$aid/docs:batch',
        body: utf8.encode(jsonEncode({
          'writes': [
            const DocWrite(op: 'set', collection: 'notes', docId: 'n1', data: {'t': 1}).toJson(),
          ],
        })),
      ),
    ).answer;
    expect(statusOf(batch).$1, 200);
    expect((await api.getDoc(aid, 'notes/n1'))!.data, {'t': 1});

    final stale = await core.call(
      CoreRequest(
        'POST',
        '/api/artifacts/$aid/versions',
        body: utf8.encode(jsonEncode(
          PublishRequest(ifVersion: 9, files: {'index.html': 'x'}).toJson(),
        )),
      ),
    ).answer;
    expect(statusOf(stale).$1, 409);
    expect((jsonOf(stale)['error']! as Map)['code'], 'conflict');
  });
}
