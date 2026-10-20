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
    final r = await core.call(const CoreRequest('GET', '/api/artifacts/a%2Fb?x=1'));
    expect(jsonOf(r), {'id': 'a/b'});
    final missing = await core.call(const CoreRequest('POST', '/api/artifacts/x'));
    expect(statusOf(missing).$1, 404);
    expect((jsonOf(missing)['error']! as Map)['code'], 'not_in_fake');
    expect(core.requests.map((r) => r.method), ['GET', 'POST']);
  });

  test('pushes blocks to live subscriptions and ends them', () async {
    final core = FakeCore();
    final a = core.subscribe();
    // A resume point is ignored, as CoreClient ignores it.
    final b = core.subscribe(lastEventId: '7');
    b.cancel();
    core.push('event: ready\n\n');
    core.end('core_closed');
    expect(await a.blocks.toList(), ['event: ready\n\n', endBlock('core_closed')]);
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
    );
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
    );
    final published = PublishResult.fromJson(jsonOf(created));
    final aid = published.artifact.id;
    expect(published.version.files['a.bin']!.contentType, 'application/x');
    expect(await api.readFileBytes(aid, 1, 'a.bin'), utf8.encode('xy'));

    final list = await core.call(const CoreRequest('GET', '/api/artifacts'));
    expect(
      [for (final a in jsonOf(list)['artifacts']! as List) ArtifactSummary.fromJson((a as Map).cast()).title],
      ['Tea'],
    );

    final item = api.sendComment(aid, 'bigger');
    final polled = await core.call(
      CoreRequest('GET', '/api/sessions/$sid/feedback?tier=wait&wait=20'),
    );
    expect(FeedbackBatch.fromJson(jsonOf(polled)).items.single.body, 'bigger');

    final commented = await core.call(
      CoreRequest(
        'POST',
        '/api/artifacts/$aid/threads/${item.threadId}/comments',
        body: utf8.encode('{"body":"and blue"}'),
      ),
    );
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
    );
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
    );
    expect(statusOf(stale).$1, 409);
    expect((jsonOf(stale)['error']! as Map)['code'], 'conflict');
  });
}
