import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

/// A 1 x 1 PNG.
final _png = Uint8List.fromList([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, //
  0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
  0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00,
  0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
  0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
  0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
]);

final _now = DateTime.utc(2026, 10, 12, 9, 20);

class _Rig {
  _Rig(this.api, this.aid);
  final FakeClaxApi api;
  final String aid;
  final previews = <int>[];
  final retries = <String>[];

  /// What onRetry answers: whether a change was queued.
  var queued = true;
}

Future<_Rig> _seeded() async {
  final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
  final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
  await api.registerAppSession(installId: 'install', pid: 1);
  return _Rig(api, aid);
}

void main() {
  Future<void> pumpDetail(
    WidgetTester tester,
    _Rig rig,
    String tid, {
    FakeVoice? voice,
    bool autoDictate = false,
    bool retry = true,
    String? label,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light),
        home: ThreadDetail(
          key: ValueKey(tid),
          api: rig.api,
          aid: rig.aid,
          tid: tid,
          voice: voice ?? FakeVoice(),
          autoDictate: autoDictate,
          onPreview: rig.previews.add,
          onRetry: retry
              ? (tid) async {
                  rig.retries.add(tid);
                  return rig.queued;
                }
              : null,
          label: label,
          now: () => _now,
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('lists every comment, the agent replies marked Clax', (
    tester,
  ) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make the title blue');
    await rig.api.replyAsAgent(rig.aid, item.threadId, 'Done: it is blue.');
    await rig.api.postOwnerComment(rig.aid, item.threadId, 'darker please');
    await pumpDetail(tester, rig, item.threadId);

    expect(find.text('make the title blue'), findsOneWidget);
    expect(find.text('Done: it is blue.'), findsOneWidget);
    expect(find.text('darker please'), findsOneWidget);
    expect(find.text('You'), findsNWidgets(2));
    expect(find.text('Clax'), findsOneWidget);
    expect(
      find.byKey(
        ValueKey(
          'comment-agent-${rig.api.threadOf(rig.aid, item.threadId).comments[1].id}',
        ),
      ),
      findsOneWidget,
    );
    // Relative time from the injected clock (comments at 09:14, now 09:20).
    expect(find.text('6 min ago'), findsNWidgets(3));
  });

  testWidgets('shows the anchor label: the shell’s words, else the anchor’s', (
    tester,
  ) async {
    final rig = await _seeded();
    final a = rig.api.sendComment(
      rig.aid,
      'shorter',
      anchor: const {'kind': 'text', 'file': 'index.html', 'quote': 'Runs'},
    );
    await pumpDetail(tester, rig, a.threadId);
    expect(find.text('“Runs”'), findsOneWidget);
    expect(find.text('The page title'), findsNothing);

    final b = rig.api.sendComment(rig.aid, 'all of it');
    await pumpDetail(tester, rig, b.threadId, label: 'the page title');
    // The title starts with a capital letter.
    expect(find.text('The page title'), findsOneWidget);

    final c = rig.api.sendComment(rig.aid, 'everything');
    await pumpDetail(tester, rig, c.threadId);
    expect(find.text('Whole app'), findsOneWidget);
  });

  testWidgets('shows the clip when the thread has one', (tester) async {
    final rig = await _seeded();
    final a = rig.api.sendComment(rig.aid, 'this', clip: _png);
    await pumpDetail(tester, rig, a.threadId);
    expect(find.byKey(const ValueKey('thread-clip')), findsOneWidget);
    expect(rig.api.callNames, contains('clip'));

    final b = rig.api.sendComment(rig.aid, 'no clip');
    rig.api.calls.clear();
    await pumpDetail(tester, rig, b.threadId);
    expect(find.byKey(const ValueKey('thread-clip')), findsNothing);
    expect(rig.api.callNames, isNot(contains('clip')));
  });

  testWidgets('Send posts the reply as the owner and clears the box', (
    tester,
  ) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await pumpDetail(tester, rig, item.threadId);

    await tester.enterText(find.byKey(const ValueKey('reply-field')), 'bluer');
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('reply-send')));
    await tester.pumpAndSettle();

    final post = rig.api.calls.lastWhere((c) => c.name == 'postOwnerComment');
    expect(post.args, {'aid': rig.aid, 'tid': item.threadId, 'body': 'bluer'});
    expect(find.text('bluer'), findsOneWidget);
    final field = tester.widget<TextField>(
      find.byKey(const ValueKey('reply-field')),
    );
    expect(field.controller!.text, isEmpty);
  });

  testWidgets('Send with an empty box posts nothing', (tester) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await pumpDetail(tester, rig, item.threadId);
    final send = tester.widget<IconButton>(
      find.byKey(const ValueKey('reply-send')),
    );
    expect(send.onPressed, isNull);
  });

  testWidgets('a reply that fails keeps its text and says why', (tester) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await pumpDetail(tester, rig, item.threadId);
    rig.api.failNext(
      'postOwnerComment',
      const ClaxApiError(
        status: 0,
        code: 'daemon_unreachable',
        message: 'connection refused',
      ),
    );
    await tester.enterText(find.byKey(const ValueKey('reply-field')), 'bluer');
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('reply-send')));
    await tester.pumpAndSettle();
    expect(find.textContaining('connection refused'), findsOneWidget);
    final field = tester.widget<TextField>(
      find.byKey(const ValueKey('reply-field')),
    );
    expect(field.controller!.text, 'bluer');
  });

  testWidgets('an unsent thread offers Send to Clax, which sends it', (
    tester,
  ) async {
    final rig = await _seeded();
    final tid = rig.api.addThread(rig.aid, 'a typo here');
    await pumpDetail(tester, rig, tid);
    expect(find.text('Not sent to Clax'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('send-to-clax')));
    await tester.pumpAndSettle();
    expect(rig.api.callNames, contains('sendThread'));
    expect(find.byKey(const ValueKey('send-to-clax')), findsNothing);
    expect(find.text('Sent to Clax'), findsOneWidget);
  });

  testWidgets('a sent thread does not offer Send to Clax', (tester) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await pumpDetail(tester, rig, item.threadId);
    expect(find.byKey(const ValueKey('send-to-clax')), findsNothing);
  });

  testWidgets('a thread the agent took reads as working', (tester) async {
    final rig = await _seeded();
    rig.api.sendComment(rig.aid, 'make it blue');
    final batch = await rig.api.pollFeedback(
      rig.api.sessionId!,
      wait: Duration.zero,
    );
    await rig.api.ackFeedback(rig.api.sessionId!, [
      for (final i in batch.items) i.commentId,
    ]);
    await pumpDetail(tester, rig, batch.items.single.threadId);
    expect(find.text('Clax is working on it'), findsOneWidget);
  });

  testWidgets('Resolve, then Reopen', (tester) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await pumpDetail(tester, rig, item.threadId);
    await tester.tap(find.byKey(const ValueKey('thread-resolve')));
    await tester.pumpAndSettle();
    expect(rig.api.callNames, contains('resolveAsOwner'));
    expect(find.text('Resolved'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('thread-reopen')));
    await tester.pumpAndSettle();
    expect(rig.api.callNames, contains('reopenThread'));
    expect(find.byKey(const ValueKey('thread-resolve')), findsOneWidget);
  });

  testWidgets('"Changed in v5" previews version 5', (tester) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    for (var n = 1; n < 4; n++) {
      await rig.api.publish(
        rig.aid,
        PublishRequest(ifVersion: n, files: {'index.html': 'v${n + 1}'}),
      );
    }
    await rig.api.publish(
      rig.aid,
      PublishRequest(
        ifVersion: 4,
        files: {'index.html': 'v5'},
        addresses: [item.threadId],
      ),
    );
    await pumpDetail(tester, rig, item.threadId);
    await tester.tap(find.text('Changed in v5'));
    await tester.pump();
    expect(rig.previews, [5]);
  });

  group('Retry', () {
    Future<(_Rig, String)> failed(String reply) async {
      final rig = await _seeded();
      final item = rig.api.sendComment(rig.aid, 'make it blue');
      await rig.api.replyAsAgent(rig.aid, item.threadId, reply);
      return (rig, item.threadId);
    }

    testWidgets('shown after a failure reply, and calls onRetry', (
      tester,
    ) async {
      final (rig, tid) = await failed(
        "Couldn't make that change: the network dropped",
      );
      await pumpDetail(tester, rig, tid);
      expect(find.text('Couldn’t make that change'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('thread-retry')));
      await tester.pump();
      expect(rig.retries, [tid]);
      expect(rig.api.callNames.where((n) => n == 'postOwnerComment'), isEmpty);
      await tester.pump();
      expect(find.text('Trying again'), findsOneWidget);
      expect(find.byKey(const ValueKey('thread-retry')), findsNothing);
    });

    testWidgets('that queued nothing keeps the failure and says so', (
      tester,
    ) async {
      final (rig, tid) = await failed("Couldn't make that change: x");
      rig.queued = false;
      await pumpDetail(tester, rig, tid);
      await tester.tap(find.byKey(const ValueKey('thread-retry')));
      await tester.pumpAndSettle();
      expect(rig.retries, [tid]);
      expect(find.text('Trying again'), findsNothing);
      expect(find.text('Couldn’t make that change'), findsOneWidget);
      expect(find.byKey(const ValueKey('thread-retry')), findsOneWidget);
      expect(
        find.text('There was nothing to run again. Reply to ask once more.'),
        findsOneWidget,
      );
    });

    testWidgets('a typographic apostrophe counts too', (tester) async {
      final (rig, tid) = await failed('Couldn’t make that change: timeout');
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('thread-retry')), findsOneWidget);
    });

    testWidgets('not shown after another reply', (tester) async {
      final (rig, tid) = await failed('Made this change in version 2.');
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('thread-retry')), findsNothing);
      expect(find.text('Clax replied'), findsOneWidget);
    });

    testWidgets('not shown when the person wrote after the failure', (
      tester,
    ) async {
      final (rig, tid) = await failed("Couldn't make that change: x");
      await rig.api.postOwnerComment(rig.aid, tid, 'try the other way');
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('thread-retry')), findsNothing);
    });

    testWidgets('not shown without a retry hook', (tester) async {
      final (rig, tid) = await failed("Couldn't make that change: x");
      await pumpDetail(tester, rig, tid, retry: false);
      expect(find.byKey(const ValueKey('thread-retry')), findsNothing);
      expect(find.text('Couldn’t make that change'), findsOneWidget);
    });
  });

  testWidgets('focusing the reply starts dictation when auto-start is on', (
    tester,
  ) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    final voice = FakeVoice(
      script: const ['darker', 'darker blue'],
      hold: true,
    );
    await pumpDetail(
      tester,
      rig,
      item.threadId,
      voice: voice,
      autoDictate: true,
    );
    expect(voice.listens, 0);
    await tester.tap(find.byKey(const ValueKey('reply-field')));
    await tester.pumpAndSettle();
    expect(voice.listens, 1);
    final field = tester.widget<TextField>(
      find.byKey(const ValueKey('reply-field')),
    );
    expect(field.controller!.text, 'darker blue');
    await tester.tap(find.byKey(const ValueKey('reply-mic')));
    await tester.pumpAndSettle();
    expect(voice.stops, 1);
  });

  testWidgets('without auto-start, the mic starts dictation', (tester) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    final voice = FakeVoice(script: const ['bluer'], hold: true);
    await pumpDetail(tester, rig, item.threadId, voice: voice);
    await tester.tap(find.byKey(const ValueKey('reply-field')));
    await tester.pumpAndSettle();
    expect(voice.listens, 0);
    await tester.enterText(find.byKey(const ValueKey('reply-field')), 'much');
    await tester.tap(find.byKey(const ValueKey('reply-mic')));
    await tester.pumpAndSettle();
    expect(voice.listens, 1);
    final field = tester.widget<TextField>(
      find.byKey(const ValueKey('reply-field')),
    );
    expect(field.controller!.text, 'much bluer');
  });

  testWidgets('a denied mic says why', (tester) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await pumpDetail(
      tester,
      rig,
      item.threadId,
      voice: FakeVoice.denied('Microphone access is off'),
    );
    await tester.tap(find.byKey(const ValueKey('reply-mic')));
    await tester.pumpAndSettle();
    expect(find.text('Microphone access is off'), findsOneWidget);
  });

  testWidgets('a new comment on the thread shows without reopening', (
    tester,
  ) async {
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await pumpDetail(tester, rig, item.threadId);
    await rig.api.replyAsAgent(rig.aid, item.threadId, 'On it.');
    rig.api.emit(
      ClaxEvent(
        type: 'thread',
        data: {'artifact_id': rig.aid, 'thread_id': item.threadId},
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('On it.'), findsOneWidget);
  });

  group('the owner', () {
    testWidgets('is You under every name, other viewers by theirs', (
      tester,
    ) async {
      final rig = await _seeded();
      rig.api.ownerName = 'Viewer';
      final item = rig.api.sendComment(rig.aid, 'make it blue');
      rig.api.ownerName = 'Alexandra';
      await rig.api.postOwnerComment(rig.aid, item.threadId, 'darker');
      final other = rig.api.sendComment(rig.aid, 'and mine', author: 'Sam');
      await pumpDetail(tester, rig, item.threadId);
      expect(find.text('You'), findsNWidgets(2));
      expect(find.text('Viewer'), findsNothing);
      expect(find.text('Alexandra'), findsNothing);

      await pumpDetail(tester, rig, other.threadId);
      expect(find.text('Sam'), findsOneWidget);
      expect(find.text('You'), findsNothing);
    });

    testWidgets('made by the first reply is read again after it', (
      tester,
    ) async {
      final rig = await _seeded();
      final item = rig.api.sendComment(rig.aid, 'mine', author: 'Sam');
      rig.api.ownerExists = false;
      await pumpDetail(tester, rig, item.threadId);
      await tester.enterText(find.byKey(const ValueKey('reply-field')), 'ok');
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('reply-send')));
      await tester.pumpAndSettle();
      expect(find.text('You'), findsOneWidget);
      expect(find.text('Sam'), findsOneWidget);
    });
  });

  testWidgets('each comment reads as one node; a failure reply says failed', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    final rig = await _seeded();
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await rig.api.replyAsAgent(
      rig.aid,
      item.threadId,
      "Couldn't make that change: timeout",
    );
    await pumpDetail(tester, rig, item.threadId);
    final t = rig.api.threadOf(rig.aid, item.threadId);
    final person = tester.getSemantics(
      find.byKey(ValueKey('comment-person-${t.comments[0].id}')),
    );
    expect(person.label, contains('You'));
    expect(person.label, contains('6 min ago'));
    expect(person.label, contains('make it blue'));
    final agent = tester.getSemantics(
      find.byKey(ValueKey('comment-agent-${t.comments[1].id}')),
    );
    expect(agent.label, contains('Clax'));
    expect(agent.label, contains('failed'));
    expect(agent.label, contains('timeout'));
    handle.dispose();
  });

  testWidgets('a thread that is gone says so', (tester) async {
    final rig = await _seeded();
    await pumpDetail(tester, rig, 'thread-404');
    expect(find.text('This thread is gone'), findsOneWidget);
  });

  group('threadStatus', () {
    ThreadView view({
      bool sent = true,
      String status = 'open',
      String? feedback = 'sent',
      List<ThreadComment> comments = const [],
    }) => ThreadView(
      id: 't',
      artifactId: 'a',
      versionN: 1,
      status: status,
      sentToAgent: sent,
      comments: comments,
      feedbackState: feedback == null ? null : FeedbackState(state: feedback),
    );
    ThreadComment c(String kind, String body) => ThreadComment(
      id: body,
      authorKind: kind,
      authorName: '',
      body: body,
      createdAt: '2026-10-12T09:14:00Z',
    );

    test('each state', () {
      final person = [c('viewer', 'x')];
      expect(
        threadStatus(view(sent: false, feedback: null, comments: person)),
        ThreadStatus.unsent,
      );
      expect(threadStatus(view(comments: person)), ThreadStatus.sent);
      expect(
        threadStatus(view(feedback: 'delivered', comments: person)),
        ThreadStatus.sent,
      );
      expect(
        threadStatus(view(feedback: 'acknowledged', comments: person)),
        ThreadStatus.working,
      );
      expect(
        threadStatus(
          view(
            feedback: 'acknowledged',
            comments: [...person, c('agent', 'ok')],
          ),
        ),
        ThreadStatus.replied,
      );
      expect(
        threadStatus(
          view(
            feedback: 'acknowledged',
            comments: [...person, c('agent', "Couldn't make that change: x")],
          ),
        ),
        ThreadStatus.failed,
      );
      expect(
        threadStatus(view(status: 'resolved', comments: person)),
        ThreadStatus.resolved,
      );
    });
  });
}
