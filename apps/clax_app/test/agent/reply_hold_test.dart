import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:clax_app/agent/reply_hold.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

/// A store whose first read waits for [gate], counting its reads.
class _SlowStore implements HeldReplies {
  final gate = Completer<void>();
  var reads = 0;
  List<HeldReply> kept = [];

  @override
  Future<List<HeldReply>> read() async {
    reads++;
    await gate.future;
    return List.of(kept);
  }

  @override
  Future<void> write(List<HeldReply> replies) async => kept = List.of(replies);
}

/// A store whose first read throws.
class _FlakyStore extends MemoryHeldReplies {
  var fail = true;

  @override
  Future<List<HeldReply>> read() async {
    if (fail) {
      fail = false;
      throw const FileSystemException('denied');
    }
    return super.read();
  }
}

void main() {
  late FakeClaxApi api;
  late String aid;
  late Set<int> public;
  late List<(int, List<String>)> publishes;
  Completer<void>? publishGate;
  String? refuse;

  /// The agent comments thread [tid] holds.
  List<String> said(String tid) => [
    for (final c in api.threadOf(aid, tid).comments)
      if (c.byAgent) c.body!,
  ];

  List<String> replies() => [
    for (final c in api.calls)
      if (c.name == 'replyAsAgent') '${c.args['tid']}: ${c.args['body']}',
  ];

  ReplyHold hold({HeldReplies? store}) => ReplyHold(
    api,
    store: store,
    publicVersion: (aid) async => public.reduce(math.max),
    publishVersion: (aid, v, addresses) async {
      publishes.add((v, List.of(addresses)));
      await publishGate?.future;
      if (refuse case final why?) return why;
      public.add(v);
      return null;
    },
  );

  setUp(() async {
    api = FakeClaxApi();
    aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'install', pid: 1);
    public = {1};
    publishes = [];
    publishGate = null;
    refuse = null;
  });

  String remote(String body) =>
      api.sendComment(aid, body, remote: 'clax.dev').threadId;

  test('a reply held while a publish is in flight is kept', () async {
    final h = hold();
    final a = remote('blue');
    final b = remote('bigger');
    await h.reply(aid, a, 'Blue.', version: 3);
    publishGate = Completer<void>();
    final publishing = h.publishAndReply(aid, a, 'Blue.', version: 2);
    await pumpEventQueue();
    // v3's reply on another thread arrives while v2 publishes.
    final held = h.reply(aid, b, 'Bigger in v3.', version: 3);
    publishGate!.complete();
    await publishing;
    await held;
    expect(
      [for (final r in await h.held(aid)) (r.threadId, r.version, r.text)],
      contains((b, 3, 'Bigger in v3.')),
    );
  });

  test('a publish under way on one app holds up no other app', () async {
    final h = hold();
    final a = remote('blue');
    final other = api.seedArtifact('Other', {'index.html': '<p>other</p>'});
    final t = api.sendComment(other, 'bigger', remote: 'clax.dev').threadId;
    await h.reply(aid, a, 'Blue.', version: 2);
    publishGate = Completer<void>();
    final publishing = h.publishAndReply(aid, a, 'Blue.', version: 2);
    await pumpEventQueue();
    expect(publishes, hasLength(1), reason: 'the publish of $aid is under way');

    // The other app's reply posts while that publish waits.
    final posting = h.reply(other, t, 'Bigger.');
    await pumpEventQueue();
    expect(replies(), ['$t: Bigger.']);
    expect(await posting, isTrue);

    publishGate!.complete();
    await publishing;
    expect(replies(), ['$t: Bigger.', '$a: Blue.']);
  });

  test('a version that does not become public posts nothing, holds the '
      'reply and says why', () async {
    final h = hold();
    final a = remote('blue');
    refuse = 'clax.dev can’t be reached right now.';
    final r = await h.publishAndReply(aid, a, 'Blue.', version: 2);
    expect(r.notPublic, 'clax.dev can’t be reached right now.');
    expect(replies(), isEmpty);
    expect(
      [for (final r in await h.held(aid)) (r.threadId, r.version, r.text)],
      [(a, 2, 'Blue.')],
    );

    // The publish that lands later releases it.
    public.add(2);
    await h.release(aid, publicVersion: 2);
    expect(replies(), ['$a: Blue.']);
    expect(await h.held(aid), isEmpty);
  });

  test('a double tap publishes and posts once', () async {
    final h = hold();
    final a = remote('blue');
    await h.reply(aid, a, 'Blue.', version: 2);
    publishGate = Completer<void>();
    final one = h.publishAndReply(aid, a, 'Blue.', version: 2);
    final two = h.publishAndReply(aid, a, 'Blue.', version: 2);
    await pumpEventQueue();
    publishGate!.complete();
    await Future.wait([one, two]);
    expect(publishes, hasLength(1));
    expect(replies(), ['$a: Blue.']);
  });

  test('concurrent first use reads the store once', () async {
    final store = _SlowStore();
    final h = hold(store: store);
    final a = remote('blue');
    final b = remote('bigger');
    final one = h.reply(aid, a, 'Blue.', version: 2);
    final two = h.reply(aid, b, 'Bigger.', version: 2);
    await pumpEventQueue();
    store.gate.complete();
    await Future.wait([one, two]);
    expect(store.reads, 1);
    expect(
      [for (final r in await h.held(aid)) r.threadId],
      unorderedEquals([a, b]),
    );
  });

  test('a post that fails after the publish keeps its reply and the rest '
      'post', () async {
    final h = hold();
    final a = remote('blue');
    final b = remote('bigger');
    await h.reply(aid, a, 'Blue.', version: 2);
    await h.reply(aid, b, 'Bigger.', version: 2);
    api.failNext(
      'replyAsAgent',
      const ClaxApiError(status: 0, code: ClaxApiError.unreachable, message: 'x'),
    );
    final r = await h.publishAndReply(aid, a, 'Blue.', version: 2);
    expect(r.failed.map((h) => h.threadId), [a]);
    expect(said(a), isEmpty);
    expect(said(b), ['Bigger.']);
    expect([for (final r in await h.held(aid)) r.threadId], [a]);
    // Released again later: posted once, not twice.
    await h.release(aid, publicVersion: 2);
    await h.release(aid, publicVersion: 2);
    expect(said(a), ['Blue.']);
    expect(said(b), ['Bigger.']);
    expect(await h.held(aid), isEmpty);
  });

  test('posting skips a thread that already has the reply', () async {
    final h = hold();
    final a = remote('blue');
    await h.reply(aid, a, 'Blue.', version: 2);
    // Posted before the app quit, the store not yet written.
    await api.replyAsAgent(aid, a, 'Blue.');
    api.calls.clear();
    public.add(2);
    await h.release(aid, publicVersion: 2);
    expect(replies(), isEmpty);
    expect(await h.held(aid), isEmpty);
  });

  test('release after an ordinary publish posts that version and earlier '
      'ones', () async {
    final h = hold();
    final a = remote('blue');
    final b = remote('bigger');
    final c = remote('round');
    await h.reply(aid, a, 'Blue.', version: 2);
    await h.reply(aid, b, 'Bigger.', version: 3);
    await h.reply(aid, c, 'Round.', version: 4);
    public.addAll({2, 3});
    final r = await h.release(aid, publicVersion: 3);
    expect(r.failed, isEmpty);
    expect(replies(), ['$a: Blue.', '$b: Bigger.']);
    expect([for (final r in await h.held(aid)) r.threadId], [c]);
  });

  test('publishAndReply on a version already public, or older, only '
      'releases', () async {
    final h = hold();
    final a = remote('blue');
    await h.reply(aid, a, 'Blue.', version: 2);
    public.addAll({2, 3});
    expect(
      (await h.publishAndReply(aid, a, 'Blue.', version: 2)).notPublic,
      isNull,
    );
    expect(publishes, isEmpty, reason: 'never publishes over a newer head');
    expect(replies(), ['$a: Blue.']);
  });

  test('publishAndReply posts the held text, matching on version', () async {
    final h = hold();
    final a = remote('blue');
    await h.reply(aid, a, 'Blue, and bigger.', version: 3);
    await h.publishAndReply(aid, a, 'Blue.', version: 2);
    expect(publishes.single.$2, isNot(contains(a)));
    expect(replies(), isEmpty, reason: "v3's reply waits for v3");
    expect([for (final r in await h.held(aid)) r.version], [3]);
  });

  test('a deleted or unlinked thread drops its reply and the rest post', () async {
    final h = hold();
    final a = remote('blue');
    final b = remote('bigger');
    final c = remote('round');
    await h.reply(aid, a, 'Blue.', version: 2);
    await h.reply(aid, b, 'Bigger.', version: 2);
    await h.reply(aid, c, 'Round.', version: 2);
    api.failNext(
      'replyAsAgent',
      const ClaxApiError(status: 409, code: 'thread_unlinked', message: 'x'),
    );
    api.failNext(
      'replyAsAgent',
      const ClaxApiError(status: 404, code: 'not_found', message: 'x'),
    );
    public.add(2);
    final r = await h.release(aid, publicVersion: 2);
    expect(r.failed, isEmpty);
    expect(r.dropped.map((h) => h.threadId), [a, b]);
    expect(said(a), isEmpty);
    expect(said(b), isEmpty);
    expect(said(c), ['Round.']);
    expect(await h.held(aid), isEmpty);
  });

  test('drop forgets a thread, or a whole app', () async {
    final h = hold();
    final a = remote('blue');
    final b = remote('bigger');
    await h.reply(aid, a, 'Blue.', version: 2);
    await h.reply(aid, b, 'Bigger.', version: 2);
    await h.drop(aid, threadId: a);
    expect([for (final r in await h.held(aid)) r.threadId], [b]);
    await h.drop(aid);
    expect(await h.held(aid), isEmpty);
    public.add(2);
    await h.release(aid, publicVersion: 2);
    expect(replies(), isEmpty);
  });

  test('an earlier agent comment with the same text is not the held reply',
      () async {
    final h = hold();
    final a = remote('blue');
    await api.replyAsAgent(aid, a, 'Done.');
    await api.postOwnerComment(aid, a, 'and red too');
    await h.reply(aid, a, 'Done.', version: 2, resolve: true);
    public.add(2);
    final r = await h.release(aid, publicVersion: 2);
    expect(r.posted.map((h) => h.threadId), [a]);
    expect(said(a), ['Done.', 'Done.']);
  });

  test("publishAndReply holds its own reply when the post fails", () async {
    final h = hold();
    final a = remote('blue');
    api.failNext(
      'replyAsAgent',
      const ClaxApiError(status: 0, code: ClaxApiError.unreachable, message: 'x'),
    );
    final r = await h.publishAndReply(aid, a, 'Blue.', version: 2);
    expect(r.failed.map((h) => h.threadId), [a]);
    expect(
      [for (final x in await h.held(aid)) (x.threadId, x.version, x.text)],
      [(a, 2, 'Blue.')],
    );
    await h.release(aid, publicVersion: 2);
    expect(said(a), ['Blue.']);
    expect(await h.held(aid), isEmpty);
  });

  test('a store read that fails is tried again', () async {
    final h = hold(store: _FlakyStore());
    final a = remote('blue');
    await expectLater(
      h.reply(aid, a, 'Blue.', version: 2),
      throwsA(isA<FileSystemException>()),
    );
    expect(await h.reply(aid, a, 'Blue.', version: 2), isFalse);
    expect([for (final x in await h.held(aid)) x.threadId], [a]);
  });
}
