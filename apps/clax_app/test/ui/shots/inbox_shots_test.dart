// The inbox and the thread page, rendered into CLAX_SHOTS_DIR (skipped
// without it): the inbox with read and unread rows of every kind, a thread
// in a run of three, and a thread waiting for the connection; on the phone
// at 100%, 130% and 200%, and in the wide and Mac forms at 100%.
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_voice.dart';
import 'rig.dart';

const _page = {'index.html': '<h1>Runs</h1>'};

void _scales(String surface, Future<void> Function(Shot s) body) {
  shots(surface, body, forms: const [phone], textScales: const [1, 1.3, 2]);
  shots(surface, body, forms: const [wide, mac]);
}

Future<void> _inbox(Shot s) async {
  final api = FakeClaxApi(clock: () => '2026-10-12T08:00:00Z');
  final runs = api.seedArtifact('Runs', _page);
  final budget = api.seedArtifact('Household budget', _page);
  final weather = api.seedArtifact('Weather on the walk to work', _page);
  final s1 = await api.registerAppSession(installId: 'i', pid: 1);
  await api.askQuestion(s1.id, [
    {
      'question': 'Which unit should distances use?',
      'header': 'Units',
      'options': [
        {'label': 'km', 'description': ''},
        {'label': 'mi', 'description': ''},
      ],
    },
  ], artifactId: runs);
  api.pushInbox('version', artifactId: budget, versionN: 4, note: 'Totals');
  api.pushInbox(
    'reply',
    artifactId: runs,
    threadId: 't1',
    body: 'Done. **Distances** now read in `km`, and the weekly total sits '
        'under the list.',
  );
  api.pushInbox('published', artifactId: weather);
  final old = api.pushInbox(
    'finished',
    artifactId: budget,
    message: 'All three fixed',
  );
  await api.markInboxRead(old.id);
  await s.tester.pumpWidget(
    s.app(
      InboxScreen(
        api: api,
        onOpen: (_) {},
        now: () => DateTime.utc(2026, 10, 12, 9, 30),
        haikus: () async => const [],
      ),
      title: 'Inbox',
    ),
  );
  await s.settle();
}

Future<void> _thread(Shot s, {bool waiting = false}) async {
  final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
  final aid = api.seedArtifact('Runs', _page);
  await api.registerAppSession(installId: 'install', pid: 1);
  final a = api.sendComment(aid, 'Make the weekly total stand out');
  final item = api.sendComment(
    aid,
    'Show the p95 for each four-hour window',
    anchor: const {
      'kind': 'element',
      'file': 'index.html',
      'name': 'Chart under ‘p95 by four-hour window’',
    },
  );
  final c = api.sendComment(aid, 'And sort the runs by distance');
  final tid = item.threadId;
  if (waiting) {
    final batch = await api.pollFeedback(api.sessionId!, wait: Duration.zero);
    await api.ackFeedback(api.sessionId!, [
      for (final i in batch.items) i.commentId,
    ]);
  } else {
    await api.replyAsAgent(
      aid,
      tid,
      'Done: each window shows its p95 as a dot over the bars.',
    );
    await api.postOwnerComment(aid, tid, 'A little bigger, please');
  }
  await s.tester.pumpWidget(
    s.app(
      ThreadDetail(
        api: api,
        aid: aid,
        tid: tid,
        voice: FakeVoice(),
        autoDictate: false,
        onPreview: (_) {},
        onRetry: (_) async => true,
        now: () => DateTime.utc(2026, 10, 12, 9, 20),
        run: [a.threadId, tid, c.threadId],
        onRun: (_) {},
        waitingForConnection: waiting ? ValueNotifier([tid]) : null,
        onTryNow: waiting ? () {} : null,
        onCancel: waiting ? () {} : null,
      ),
      title: 'Runs',
    ),
  );
  await s.settle();
}

void main() {
  _scales('inbox', (s) async {
    await _inbox(s);
    await s.take('inbox');
  });
  _scales('inbox-thread', (s) async {
    await _thread(s);
    await s.take('inbox-thread');
  });
  _scales('inbox-thread-waiting', (s) async {
    await _thread(s, waiting: true);
    await s.take('inbox-thread-waiting');
  });
}
