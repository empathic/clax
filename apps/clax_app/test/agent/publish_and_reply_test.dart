import 'dart:io';
import 'dart:math' as math;

import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/reply_hold.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const _page = '<!doctype html><title>Runs</title><button>Save</button>';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late String aid;
  late HeldReplies store;

  /// Each publish asked for, with how many agent replies and resolves had
  /// been sent when it was asked.
  late List<
    ({String aid, int version, List<String> addresses, int replies, int resolves})
  >
  publishes;

  /// The local versions that are public.
  late Set<int> public;

  /// What a publish answers.
  late bool publishOk;

  late FeedbackWorker? started;

  List<String> replies() => [
    for (final c in api.calls)
      if (c.name == 'replyAsAgent') '${c.args['tid']}: ${c.args['body']}',
  ];
  int resolves() => api.callNames.where((n) => n == 'resolveAsAgent').length;

  ReplyHold hold() => ReplyHold(
    api,
    store: store,
    publicVersion: (aid) async => public.reduce(math.max),
    publishVersion: (aid, version, addresses) async {
      publishes.add((
        aid: aid,
        version: version,
        addresses: List.of(addresses),
        replies: replies().length,
        resolves: resolves(),
      ));
      if (!publishOk) return 'The publish was refused.';
      public.add(version);
      return null;
    },
  );

  FeedbackWorker worker([List<List<ScriptedCall>> script = const []]) =>
      FeedbackWorker(
        api: api,
        loop: AgentLoop(
          api: api,
          runner: () => HeldRunner(script),
          prompts: PromptBuilder(rootBundle),
          model: () => 'claude-haiku-5-5',
          timings: RecordingTimings(),
          inspector: FakeInspector(),
          baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
          ledger: MemoryChangeLedger(),
          asked: MemoryAskedQuestions(),
          clock: StepClock(),
          replies: hold(),
        ),
        session: () async =>
            (await api.registerAppSession(installId: 'install', pid: 1)).id,
        delay: (_) async {},
      );

  setUp(() async {
    api = FakeClaxApi();
    aid = api.seedArtifact('Runs', {'index.html': _page});
    await api.registerAppSession(installId: 'install', pid: 1);
    store = MemoryHeldReplies();
    publishes = [];
    public = {1};
    publishOk = true;
    started = null;
  });

  tearDown(() async {
    await started?.stop();
    api.releasePolls();
  });

  test('reply_without_new_version_posts_at_once', () async {
    final w = worker();
    final remote = api.sendComment(aid, 'why blue?', remote: 'clax.dev');
    final local = api.sendComment(aid, 'bigger');

    expect(await w.replyOnThread(aid, remote.threadId, 'It matches.'), isTrue);
    // A version already public, and a thread with no remote twin, wait for
    // nothing either.
    expect(
      await w.replyOnThread(aid, remote.threadId, 'Still v1.', version: 1),
      isTrue,
    );
    expect(
      await w.replyOnThread(aid, local.threadId, 'Bigger now.', version: 2),
      isTrue,
    );

    expect(replies(), [
      '${remote.threadId}: It matches.',
      '${remote.threadId}: Still v1.',
      '${local.threadId}: Bigger now.',
    ]);
    expect(await w.heldReplies(aid), isEmpty);
    expect(publishes, isEmpty);
  });

  test('publish_and_reply_publishes_then_replies', () async {
    final w = worker();
    final a = api.sendComment(aid, 'make it blue', remote: 'clax.dev');
    final b = api.sendComment(aid, 'and bigger', remote: 'clax.dev');

    // The fix is local version 2, not public yet: both replies wait.
    expect(
      await w.replyOnThread(aid, a.threadId, 'Blue in v2.', version: 2),
      isFalse,
    );
    expect(
      await w.replyOnThread(aid, b.threadId, 'Bigger in v2.', version: 2),
      isFalse,
    );
    expect(replies(), isEmpty, reason: 'held until v2 is public');
    expect(
      [
        for (final h in await w.heldReplies(aid))
          (h.threadId, h.version, h.text),
      ],
      [(a.threadId, 2, 'Blue in v2.'), (b.threadId, 2, 'Bigger in v2.')],
    );

    // A refused publish posts nothing and keeps the replies held.
    publishOk = false;
    expect(
      (await w.publishAndReply(aid, a.threadId, 'Blue in v2.', version: 2))
          .notPublic,
      'The publish was refused.',
    );
    expect(replies(), isEmpty);
    expect(await w.heldReplies(aid), hasLength(2));

    // Publish v2 and reply: the publish lists every thread v2 answers, and
    // comes before any reply; each thread gets its held text.
    publishOk = true;
    expect(
      (await w.publishAndReply(aid, a.threadId, 'Blue now, in v2.', version: 2))
          .notPublic,
      isNull,
    );
    expect(publishes.last.aid, aid);
    expect(publishes.last.version, 2);
    expect(publishes.last.addresses, unorderedEquals([a.threadId, b.threadId]));
    expect(publishes.last.replies, 0, reason: 'published before replying');
    expect(replies(), [
      '${a.threadId}: Blue in v2.',
      '${b.threadId}: Bigger in v2.',
    ], reason: 'the held text, not the text the caller passed');
    expect(await w.heldReplies(aid), isEmpty);
  });

  test('a change on a linked thread holds its reply until its version is '
      'public', () async {
    final w = started = worker([
      changeCalls(reply: 'Kept.', edits: [('Save', 'Keep')]),
    ])..start();
    final item = api.sendComment(aid, 'say Keep', remote: 'clax.dev');
    await until(
      () => api.currentVersion(aid) == 2 && api.waitingPolls == 1 && w.isIdle,
      reason: 'the change landed v2 and the worker is idle',
    );

    expect(replies(), isEmpty, reason: 'nothing reaches clax.dev before v2');
    expect(resolves(), 0, reason: 'nor the resolve');
    final held = await w.heldReplies(aid);
    expect(
      [for (final h in held) (h.threadId, h.version, h.text, h.resolve)],
      [(item.threadId, 2, 'Kept.', true)],
    );

    publishOk = false;
    expect(
      (await w.publishAndReply(aid, item.threadId, 'Kept.', version: 2))
          .notPublic,
      'The publish was refused.',
    );
    expect(replies(), isEmpty, reason: 'a refused publish posts nothing');

    publishOk = true;
    expect(
      (await w.publishAndReply(aid, item.threadId, 'Kept.', version: 2))
          .notPublic,
      isNull,
    );
    expect(publishes.last.version, 2);
    expect(publishes.last.addresses, [item.threadId]);
    expect(publishes.last.replies, 0);
    expect(publishes.last.resolves, 0);
    expect(replies(), ['${item.threadId}: Kept.']);
    expect(resolves(), 1, reason: 'resolved once the reply is posted');
  });

  test('recovery keeps a held reply\'s own text and resolve', () async {
    final w = started = worker([
      changeCalls(reply: 'Kept.', edits: [('Save', 'Keep')]),
    ])..start();
    final item = api.sendComment(aid, 'say Keep', remote: 'clax.dev');
    await until(
      () => api.currentVersion(aid) == 2 && api.waitingPolls == 1 && w.isIdle,
      reason: 'the change landed v2 and the worker is idle',
    );
    await w.stop();
    api.releasePolls();

    // The app starts again with no ledger record: recovery finds the
    // thread stranded on v2.
    final again = started = worker()..start();
    await until(
      () => api.waitingPolls == 1 && again.isIdle,
      reason: 'recovered',
    );
    expect(
      [
        for (final h in await again.heldReplies(aid))
          (h.threadId, h.version, h.text, h.resolve),
      ],
      [(item.threadId, 2, 'Kept.', true)],
    );
    expect(replies(), isEmpty);
  });

  test('a change on a thread with no remote twin replies at once', () async {
    final w = started = worker([
      changeCalls(reply: 'Kept.', edits: [('Save', 'Keep')]),
    ])..start();
    final item = api.sendComment(aid, 'say Keep');
    await until(
      () => api.currentVersion(aid) == 2 && api.waitingPolls == 1 && w.isIdle,
      reason: 'the change landed v2 and the worker is idle',
    );
    expect(replies(), ['${item.threadId}: Kept.']);
    expect(await w.heldReplies(aid), isEmpty);
  });

  test('held replies outlive the app in their file', () async {
    final dir = await Directory.systemTemp.createTemp('held-replies');
    addTearDown(() => dir.delete(recursive: true));
    final file = File('${dir.path}/held-replies.json');
    store = FileHeldReplies(file);
    final t = api.sendComment(aid, 'make it blue', remote: 'clax.dev');
    expect(
      await hold().reply(aid, t.threadId, 'Blue.', version: 2, resolve: true),
      isFalse,
    );

    // A new run of the app reads them back.
    store = FileHeldReplies(file);
    final again = hold();
    expect(
      [
        for (final h in await again.held(aid))
          (h.threadId, h.version, h.text, h.resolve),
      ],
      [(t.threadId, 2, 'Blue.', true)],
    );
    expect(
      (await again.publishAndReply(aid, t.threadId, 'Blue.', version: 2))
          .notPublic,
      isNull,
    );
    expect(replies(), ['${t.threadId}: Blue.']);
    expect(await FileHeldReplies(file).read(), isEmpty);

    // A file that does not parse is set aside, never overwritten.
    await file.writeAsString('not json');
    expect(await FileHeldReplies(file).read(), isEmpty);
    expect(await File('${file.path}.bad-1').readAsString(), 'not json');
    expect(await file.exists(), isFalse);
    await file.writeAsString('[');
    expect(await FileHeldReplies(file).read(), isEmpty);
    expect(await File('${file.path}.bad-2').readAsString(), '[');
  });
}
