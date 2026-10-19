import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/foundation.dart';
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
    ValueListenable<List<String>>? waiting,
    VoidCallback? onCancel,
    VoidCallback? onTryNow,
    bool Function(String tid)? isStopped,
    bool developer = false,
    TargetPlatform? platform,
  }) async {
    final dev = ValueNotifier(developer);
    addTearDown(dev.dispose);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, platform ?? defaultTargetPlatform),
        builder: (context, child) =>
            DeveloperScope(notifier: dev, child: child!),
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
          waitingForConnection: waiting,
          onCancel: onCancel,
          onTryNow: onTryNow,
          isStopped: isStopped,
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

    // An anchor's spaced label reads in place of its run-together quote.
    expect(
      anchorWords(const {
        'kind': 'element',
        'quote': 'Mon5 km',
        'label': 'Mon 5 km',
      }),
      '“Mon 5 km”',
    );
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
      expect(find.text('The network dropped'), findsOneWidget);
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
      expect(find.text('X'), findsOneWidget);
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

    testWidgets('a limit stop offers Keep going, which calls onRetry, with '
        'the reason without the agent\'s words', (tester) async {
      final (rig, tid) = await failed(
        limitFailureReply(
          '10 turns',
          leftUsd: 6,
          agentSaid: 'I ran out of room in this run',
        ),
      );
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('thread-retry')), findsNothing);
      expect(
        find.text(
          'It was too big for one go — it needed more than 10 steps. '
          '$keepGoingHint',
        ),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const ValueKey('thread-keep-going')));
      await tester.pump();
      await tester.pump();
      expect(rig.retries, [tid]);
      expect(find.text('Keeping going'), findsOneWidget);
    });

    testWidgets('a limit stop that published part of the change offers Keep '
        'going too', (tester) async {
      final (rig, tid) = await failed(
        limitLandedReply('120 seconds', leftUsd: 2, agentSaid: 'Adding it'),
      );
      await pumpDetail(tester, rig, tid);
      expect(find.text('Clax replied'), findsOneWidget);
      expect(find.byKey(const ValueKey('thread-keep-going')), findsOneWidget);
    });

    testWidgets('a change that spent its \$6 offers no Keep going', (
      tester,
    ) async {
      final (rig, tid) = await failed(
        limitFailureReply('10 turns', leftUsd: 0),
      );
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('thread-keep-going')), findsNothing);
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
      expect(find.text('X'), findsOneWidget);
    });
  });

  group('A failed change', () {
    const progress = LimitProgress(
      stops: 2,
      spentUsd: 2.1,
      notes: [
        'Adding the chart canvas to the stats screen.',
        'Wiring the run-type filter to the chart.',
        'Fixing the failing week-boundary test.',
      ],
      changed: [
        (path: 'index.html', added: 42, removed: 10),
        (path: 'app.js', added: 8, removed: 0),
      ],
    );

    Future<(_Rig, String)> failed(String reply) async {
      final rig = await _seeded();
      final item = rig.api.sendComment(rig.aid, 'add a weekly chart');
      await rig.api.replyAsAgent(rig.aid, item.threadId, reply);
      return (rig, item.threadId);
    }

    /// Every fill drawn in the thread.
    Iterable<Color?> fills(WidgetTester tester) sync* {
      for (final e in find
          .descendant(
            of: find.byKey(const ValueKey('thread-list')),
            matching: find.byWidgetPredicate((_) => true),
          )
          .evaluate()) {
        final w = e.widget;
        if (w is Material) yield w.color;
        if (w is ColoredBox) yield w.color;
        if (w is DecoratedBox && w.decoration is BoxDecoration) {
          yield (w.decoration as BoxDecoration).color;
        }
        if (w is Container) yield w.color;
      }
    }

    testWidgets('shows once, as Clax’s reply, with no banner above it and no '
        'error fill', (tester) async {
      final (rig, tid) = await failed(
        "Couldn't make that change: this app may only reach "
        "api.weather.example, so it can't reach strava.com.",
      );
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('thread-status-failed')), findsNothing);
      expect(find.text('Couldn’t make that change'), findsNothing);
      expect(find.textContaining('strava.com'), findsOneWidget);
      expect(find.byKey(const ValueKey('failure-reply')), findsOneWidget);
      expect(find.byKey(const ValueKey('thread-retry')), findsOneWidget);
      expect(fills(tester), isNot(contains(ClaxTokens.light.dangerTint)));
    });

    testWidgets('keeps the cost, files and notes under Details, closed until '
        'opened', (tester) async {
      final (rig, tid) = await failed(
        limitFailureReply('10 turns', leftUsd: 6 - 2.1, progress: progress),
      );
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('thread-keep-going')), findsOneWidget);
      expect(find.textContaining(r'$2.10 of $6'), findsNothing);
      expect(find.text('Wiring the run-type filter to the chart.'), findsNothing);
      await tester.tap(find.byKey(const ValueKey('failure-details')));
      await tester.pumpAndSettle();
      expect(find.textContaining(r'$2.10 of $6'), findsOneWidget);
      expect(find.text('Stopped twice · \$2.10 of \$6 used'), findsOneWidget);
      expect(find.text('index.html'), findsOneWidget);
      expect(find.text('+42 −10'), findsOneWidget);
      // The notes are a bulleted list, with no Markdown dash left.
      expect(find.text('•'), findsNWidgets(3));
      expect(find.text('Wiring the run-type filter to the chart.'), findsOneWidget);
      expect(find.textContaining('- '), findsNothing);
      expect(fills(tester), isNot(contains(ClaxTokens.light.dangerTint)));
      await tester.tap(find.byKey(const ValueKey('failure-details')));
      await tester.pumpAndSettle();
      expect(find.textContaining(r'$2.10 of $6'), findsNothing);
    });

    testWidgets('offers no Retry once the change has spent its limit', (
      tester,
    ) async {
      final (rig, tid) = await failed(
        limitFailureReply(
          '10 turns',
          leftUsd: 0,
          progress: const LimitProgress(stops: 4, spentUsd: 6),
        ),
      );
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('failure-reply')), findsOneWidget);
      expect(find.byKey(const ValueKey('thread-retry')), findsNothing);
      expect(find.byKey(const ValueKey('thread-keep-going')), findsNothing);
      expect(find.byKey(const ValueKey('failure-details')), findsOneWidget);
    });

    testWidgets('shows a raw error as one plain line, the raw text only with '
        'Developer tools on, last in Details', (tester) async {
      const raw = 'The Claude API said: overloaded (529)';
      final (rig, tid) = await failed("Couldn't make that change: $raw");
      await pumpDetail(tester, rig, tid);
      expect(
        find.text('Claude is busy right now. Try again in a minute.'),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('failure-details')), findsNothing);
      expect(find.textContaining('529'), findsNothing);

      await pumpDetail(tester, rig, tid, developer: true);
      await tester.tap(find.byKey(const ValueKey('failure-details')));
      await tester.pumpAndSettle();
      expect(find.text(raw), findsOneWidget);
    });

    testWidgets('says Click Keep going on a Mac, for a reply stored with Tap', (
      tester,
    ) async {
      final (rig, tid) = await failed(
        limitFailureReply('10 turns', leftUsd: 5, progress: progress),
      );
      expect(
        rig.api.threadOf(rig.aid, tid).comments.last.body,
        contains('Tap Keep going'),
      );
      await pumpDetail(tester, rig, tid, platform: TargetPlatform.macOS);
      expect(find.textContaining('Click Keep going'), findsOneWidget);
      expect(find.textContaining('Tap Keep going'), findsNothing);
      expect(find.byKey(const ValueKey('thread-keep-going')), findsOneWidget);
    });

    testWidgets('a limit stop that published part of the change keeps its '
        'progress under Details too', (tester) async {
      final (rig, tid) = await failed(
        limitLandedReply('10 turns', leftUsd: 5, progress: progress),
      );
      await pumpDetail(tester, rig, tid);
      expect(find.byKey(const ValueKey('failure-reply')), findsNothing);
      expect(find.textContaining(r'$2.10 of $6'), findsNothing);
      await tester.tap(find.byKey(const ValueKey('failure-details')));
      await tester.pumpAndSettle();
      expect(find.textContaining(r'$2.10 of $6'), findsOneWidget);
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
    expect(agent.label, contains('Timeout'));
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
    ThreadComment c(String kind, String body, {String? harness}) =>
        ThreadComment(
          id: body,
          authorKind: kind,
          authorName: '',
          body: body,
          createdAt: '2026-10-12T09:14:00Z',
          viaHarness: harness ?? (kind == 'agent' ? 'app' : null),
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
        threadStatus(
          view(
            feedback: 'acknowledged',
            comments: [
              ...person,
              c('agent', "Couldn't make that change: x", harness: 'claude'),
            ],
          ),
        ),
        ThreadStatus.replied,
        reason: 'another harness\'s reply is not the app\'s failure',
      );
      expect(
        threadStatus(view(status: 'resolved', comments: person)),
        ThreadStatus.resolved,
      );
    });
  });

  group('landing', () {
    /// A thread long enough to scroll on a phone: [n] rounds of a comment
    /// and a reply; answers the thread and the reply IDs.
    Future<(String, List<String>)> longThread(_Rig rig, int n) async {
      final item = rig.api.sendComment(rig.aid, 'round 0');
      final replies = <String>[];
      for (var i = 0; i < n; i++) {
        await rig.api.replyAsAgent(
          rig.aid,
          item.threadId,
          'Reply $i: ${'a long answer that wraps over lines ' * 4}',
        );
        replies.add(rig.api.threadOf(rig.aid, item.threadId).comments.last.id);
        await rig.api.postOwnerComment(
          rig.aid,
          item.threadId,
          'round ${i + 1}',
        );
      }
      return (item.threadId, replies);
    }

    Future<void> pumpFocus(
      WidgetTester tester,
      _Rig rig,
      String tid, {
      String? focus,
      bool latestReply = false,
      List<String> run = const [],
      ValueChanged<String>? onRun,
    }) async {
      tester.view.physicalSize = const Size(360, 700) * 3;
      tester.view.devicePixelRatio = 3;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          home: ThreadDetail(
            api: rig.api,
            aid: rig.aid,
            tid: tid,
            voice: FakeVoice(),
            autoDictate: false,
            onPreview: (_) {},
            onRetry: null,
            now: () => _now,
            focusComment: focus,
            focusLatestReply: latestReply,
            run: run,
            onRun: onRun,
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    double offset(WidgetTester tester) => tester
        .state<ScrollableState>(
          find.descendant(
            of: find.byKey(const ValueKey('thread-list')),
            matching: find.byType(Scrollable),
          ),
        )
        .position
        .pixels;

    double maxOffset(WidgetTester tester) => tester
        .state<ScrollableState>(
          find.descendant(
            of: find.byKey(const ValueKey('thread-list')),
            matching: find.byType(Scrollable),
          ),
        )
        .position
        .maxScrollExtent;

    BoxDecoration glowOf(WidgetTester tester, String id) =>
        tester
                .widget<AnimatedContainer>(
                  find
                      .descendant(
                        of: find.byKey(ValueKey('comment-agent-$id')),
                        matching: find.byType(AnimatedContainer),
                      )
                      .first,
                )
                .decoration!
            as BoxDecoration;

    testWidgets('without a focus it lands at the newest comment', (
      tester,
    ) async {
      final rig = await _seeded();
      final (tid, _) = await longThread(rig, 6);
      await pumpFocus(tester, rig, tid);
      expect(maxOffset(tester), greaterThan(0), reason: 'it scrolls');
      expect(offset(tester), maxOffset(tester));
      expect(find.text('round 6'), findsOneWidget);
    });

    testWidgets('a focused comment is scrolled to and lit, then fades', (
      tester,
    ) async {
      final rig = await _seeded();
      final (tid, replies) = await longThread(rig, 6);
      await pumpFocus(tester, rig, tid, focus: replies[1]);
      final box = tester.getRect(
        find.byKey(ValueKey('comment-agent-${replies[1]}')),
      );
      expect(box.top, greaterThanOrEqualTo(0));
      expect(box.bottom, lessThan(700));
      expect(offset(tester), lessThan(maxOffset(tester)));
      expect(glowOf(tester, replies[1]).color!.a, greaterThan(0));
      expect(glowOf(tester, replies[3]).color!.a, 0);

      await tester.pump(ThreadDetail.glowFor);
      await tester.pumpAndSettle();
      expect(glowOf(tester, replies[1]).color!.a, 0, reason: 'faded');
    });

    testWidgets('focusLatestReply waits for the reply to the last comment, '
        'and lights it when it arrives', (tester) async {
      final rig = await _seeded();
      final (tid, replies) = await longThread(rig, 3);
      await pumpFocus(tester, rig, tid, latestReply: true);
      expect(
        glowOf(tester, replies.last).color!.a,
        0,
        reason: 'an earlier reply is not the one asked for',
      );
      await rig.api.replyAsAgent(rig.aid, tid, 'Here it is.');
      final reply = rig.api.threadOf(rig.aid, tid).comments.last.id;
      rig.api.emit(
        ClaxEvent(
          type: 'comment',
          data: {'artifact_id': rig.aid, 'thread_id': tid},
        ),
      );
      await tester.pumpAndSettle();
      expect(glowOf(tester, reply).color!.a, greaterThan(0));
      await tester.pump(ThreadDetail.glowFor);
      await tester.pumpAndSettle();
    });

    testWidgets('a focused comment missing from the first read is lit once '
        'a later read holds it', (tester) async {
      final api = _StaleApi(clock: () => '2026-10-12T09:14:00Z');
      final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
      await api.registerAppSession(installId: 'install', pid: 1);
      final rig = _Rig(api, aid);
      final (tid, replies) = await longThread(rig, 2);
      await api.replyAsAgent(aid, tid, 'Later.');
      final id = api.threadOf(aid, tid).comments.last.id;
      api.stale = true;
      await pumpFocus(tester, rig, tid, focus: id);
      expect(api.staleServed, isTrue, reason: 'the first read lacked it');
      await tester.pumpAndSettle();
      expect(glowOf(tester, id).color!.a, greaterThan(0));
      expect(glowOf(tester, replies.last).color!.a, 0);
      await tester.pump(ThreadDetail.glowFor);
      await tester.pumpAndSettle();
    });

    testWidgets('a run of threads shows its place and moves to the next', (
      tester,
    ) async {
      final rig = await _seeded();
      final a = rig.api.sendComment(rig.aid, 'one');
      final b = rig.api.sendComment(rig.aid, 'two');
      final c = rig.api.sendComment(rig.aid, 'three');
      final opened = <String>[];
      final run = [a.threadId, b.threadId, c.threadId];
      await pumpFocus(tester, rig, b.threadId, run: run, onRun: opened.add);
      expect(find.text('2 of 3 threads in this change'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('thread-run-next')));
      await tester.tap(find.byKey(const ValueKey('thread-run-previous')));
      expect(opened, [c.threadId, a.threadId]);

      await pumpFocus(tester, rig, a.threadId, run: [a.threadId]);
      expect(find.byKey(const ValueKey('thread-run-place')), findsNothing);
    });
  });

  testWidgets('a working thread whose change waits for the connection says '
      'so, offers Cancel, and reads as working once it is back', (
    tester,
  ) async {
    final rig = await _seeded();
    rig.api.sendComment(rig.aid, 'make it blue');
    final batch = await rig.api.pollFeedback(
      rig.api.sessionId!,
      wait: Duration.zero,
    );
    await rig.api.ackFeedback(rig.api.sessionId!, [
      for (final i in batch.items) i.commentId,
    ]);
    final tid = batch.items.single.threadId;
    final waiting = ValueNotifier<List<String>>([tid]);
    var tries = 0;
    await pumpDetail(
      tester,
      rig,
      tid,
      waiting: waiting,
      onCancel: () {},
      onTryNow: () => tries++,
    );
    expect(find.text('Waiting for connection'), findsOneWidget);
    expect(find.text('Clax is working on it'), findsNothing);
    await tester.tap(find.byKey(const ValueKey('thread-try-now')));
    expect(tries, 1);

    waiting.value = const [];
    await tester.pump();
    expect(find.text('Waiting for connection'), findsNothing);
    expect(find.text('Clax is working on it'), findsOneWidget);
    expect(find.byKey(const ValueKey('thread-cancel')), findsNothing);
  });

  testWidgets('Cancel on a waiting thread leaves it reading Cancelled, with '
      'Send again, never working', (tester) async {
    final rig = await _seeded();
    rig.api.sendComment(rig.aid, 'make it blue');
    final batch = await rig.api.pollFeedback(
      rig.api.sessionId!,
      wait: Duration.zero,
    );
    await rig.api.ackFeedback(rig.api.sessionId!, [
      for (final i in batch.items) i.commentId,
    ]);
    final tid = batch.items.single.threadId;
    final stopped = <String>{};
    // The worker's list still names it until its change ends.
    final waiting = ValueNotifier<List<String>>([tid]);
    await pumpDetail(
      tester,
      rig,
      tid,
      waiting: waiting,
      onCancel: () => stopped.add(tid),
      isStopped: stopped.contains,
    );
    await tester.tap(find.byKey(const ValueKey('thread-cancel')));
    await tester.pump();
    expect(find.text('Cancelled'), findsOneWidget);
    expect(find.text('Clax is working on it'), findsNothing);
    expect(find.text('Waiting for connection'), findsNothing);

    await tester.tap(find.byKey(const ValueKey('thread-send-again')));
    await tester.pump();
    expect(rig.retries, [tid]);
    stopped.remove(tid);
    waiting.value = const [];
    await tester.pump();
    expect(find.text('Clax is working on it'), findsOneWidget);
  });
}

/// A fake whose next thread read, once [stale] is set, lacks each thread's
/// newest comment.
class _StaleApi extends FakeClaxApi {
  _StaleApi({super.clock});
  var stale = false;
  var staleServed = false;

  @override
  Future<List<ThreadView>> threads(String aid) async {
    final all = await super.threads(aid);
    if (!stale) return all;
    stale = false;
    staleServed = true;
    return [
      for (final t in all)
        ThreadView(
          id: t.id,
          artifactId: t.artifactId,
          versionN: t.versionN,
          status: t.status,
          sentToAgent: t.sentToAgent,
          comments: t.comments.sublist(0, t.comments.length - 1),
          anchor: t.anchor,
          createdAt: t.createdAt,
        ),
    ];
  }
}
