// The thread page with a failed change, rendered in every form and theme
// into CLAX_SHOTS_DIR (skipped without it): a plain failure, and a limit
// stop offering Keep going, each with Details closed and open; and the
// plain failure of a raw error with Developer tools on; and a thread a
// stranger opened on clax.dev, whose comments wait on the person's
// approval, with the public copy's comments off.
import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_voice.dart';
import 'rig.dart';

const _progress = LimitProgress(
  stops: 2,
  spentUsd: 2.1,
  notes: [
    'Adding the chart canvas to the stats screen.',
    'Wiring the **run-type** filter to the chart.',
    'Fixing the failing `week-boundary` test.',
  ],
  changed: [
    (path: 'index.html', added: 42, removed: 10),
    (path: 'app.js', added: 8, removed: 0),
    (path: 'tests/week.test.js', added: 21, removed: 0),
  ],
);

Future<void> _thread(
  Shot s,
  String reply, {
  bool open = false,
  bool developer = false,
}) async {
  final api = FakeClaxApi(clock: () => '2026-10-16T09:14:00Z');
  final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
  await api.registerAppSession(installId: 'install', pid: 1);
  final item = api.sendComment(
    aid,
    'Add a weekly chart with filters by run type',
    queue: false,
  );
  await api.replyAsAgent(aid, item.threadId, reply);
  final dev = ValueNotifier(developer);
  addTearDown(dev.dispose);
  await s.tester.pumpWidget(
    DeveloperScope(
      notifier: dev,
      child: s.app(
        ThreadDetail(
          api: api,
          aid: aid,
          tid: item.threadId,
          voice: FakeVoice(),
          autoDictate: false,
          onPreview: (_) {},
          onRetry: (_) async => true,
          label: 'the weekly chart',
          now: () => DateTime.utc(2026, 10, 16, 9, 20),
        ),
      ),
    ),
  );
  await s.settle();
  if (open) {
    await s.tester.tap(find.byKey(const ValueKey('failure-details')));
    await s.settle();
  }
}

void main() {
  const plain =
      "Couldn't make that change: this app may only reach "
      "api.weather.example, so it can't reach strava.com.\n\n"
      'The agent said: The page asks for strava.com/api/v3/athlete, which '
      'the app’s network list does not name.';
  final limit = limitFailureReply(
    '10 turns',
    leftUsd: changeCapUsd - 2.1,
    progress: _progress,
  );
  final spent = limitFailureReply(
    '10 turns',
    leftUsd: 0,
    progress: const LimitProgress(stops: 4, spentUsd: 6, notes: ['Done.']),
  );

  shots('thread-failed', (s) async {
    await _thread(s, plain);
    await s.take('thread-failed');
  });
  shots('thread-failed-details', (s) async {
    await _thread(s, plain, open: true);
    await s.take('thread-failed-details');
  });
  shots('thread-failed-raw', (s) async {
    await _thread(
      s,
      "Couldn't make that change: The Claude API said: overloaded (529)",
      open: true,
      developer: true,
    );
    await s.take('thread-failed-raw');
  });
  shots('thread-limit', (s) async {
    await _thread(s, limit);
    await s.take('thread-limit');
  });
  shots('thread-limit-details', (s) async {
    await _thread(s, limit, open: true);
    await s.take('thread-limit-details');
  });
  shots('thread-withheld', (s) async {
    final api = FakeClaxApi(clock: () => '2026-10-16T09:14:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    final sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
    final t = api.sendComment(
      aid,
      'Add a weekly chart with filters by run type',
      author: 'Bob Example-Longname (on clax.dev)',
      remote: 'clax.dev',
      remoteAuthor: 'remote:clax.dev#u_bob',
      queue: false,
    );
    await s.tester.runAsync(() => api.askApproval(sid, threadId: t.threadId));
    await s.tester.pumpWidget(
      s.app(
        ThreadDetail(
          api: api,
          aid: aid,
          tid: t.threadId,
          voice: FakeVoice(),
          autoDictate: false,
          onPreview: (_) {},
          onRetry: (_) async => true,
          label: 'the weekly chart',
          replies: false,
          now: () => DateTime.utc(2026, 10, 16, 9, 20),
        ),
      ),
    );
    await s.settle();
    await s.take('thread-withheld');
  }, textScales: const [1, 2]);
  shots('thread-limit-spent', (s) async {
    await _thread(s, spent);
    await s.take('thread-limit-spent');
  });
}
