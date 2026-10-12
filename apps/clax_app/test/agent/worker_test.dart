import 'dart:async';

import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'loop_support.dart';

const page = '<!doctype html><title>Runs</title><button>Save</button>';

const unreachable = ClaxApiError(
  status: 0,
  code: 'daemon_unreachable',
  message: 'no connection',
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late HeldBackend backend;
  late FeedbackWorker worker;
  late List<WorkerEvent> events;
  late int registrations;
  late List<Duration> delays;

  FeedbackWorker workerWith(List<Object> script) {
    backend = HeldBackend(script);
    final loop = AgentLoop(
      api: api,
      backend: () => backend,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
    );
    worker = FeedbackWorker(
      api: api,
      loop: loop,
      session: () async {
        registrations++;
        return (await api.registerAppSession(installId: 'install', pid: 1)).id;
      },
      delay: (d) async => delays.add(d),
    );
    worker.events.listen(events.add);
    return worker;
  }

  List<String> publishedOn(String aid) => [
    for (final c in api.calls)
      if (c.name == 'publish' && c.args['aid'] == aid)
        '${(c.args['request']! as PublishRequest).ifVersion}',
  ];

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  setUp(() {
    api = FakeClaxApi();
    events = [];
    registrations = 0;
    delays = [];
  });

  tearDown(() async {
    await worker.stop();
    api.releasePolls();
  });

  test('second_batch_runs_on_the_landed_version', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Red.', edits: [edit('<button>', '<button class=red>')]),
    ]).start();
    await idle();
    backend.gate = Completer<void>();
    api.sendComment(aid, 'Say Keep');
    await backend.arrived.future;
    final second = api.sendComment(aid, 'Make it red');
    await until(() => api.waitingPolls == 1, reason: 'second batch taken');
    expect(backend.seen, hasLength(1), reason: 'one change at a time');
    backend.gate!.complete();
    backend.gate = null;
    await idle();

    expect(publishedOn(aid), ['1', '2']);
    expect(backend.seen[1].userText, contains('Keep'));
    expect(
      api.filesOf(aid, 3)['index.html'],
      '<!doctype html><title>Runs</title><button class=red>Keep</button>',
    );
    expect(api.threadOf(aid, second.threadId).comments.last.body, 'Red.');
    expect(
      [
        for (final e in events)
          switch (e) {
            WorkerWorking() => 'working',
            WorkerLanded(:final version) => 'landed $version',
            WorkerReplied() => 'replied',
            WorkerFailed() => 'failed',
            WorkerCancelled() => 'cancelled',
            WorkerInterrupted() => 'interrupted',
            WorkerProgress() => 'progress',
          },
      ],
      ['working', 'landed 2', 'working', 'landed 3'],
    );
  });

  test('a change\'s progress arrives between working and landed, with its '
      'threads', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      WithProgress(const [
        PartialEdit('index.html', 'Ke'),
        OutputTokens(30),
      ], change(reply: 'Keep.', edits: [edit('Save', 'Keep')])),
    ]).start();
    await idle();
    final item = api.sendComment(aid, 'Say Keep');
    await until(() => events.any((e) => e is WorkerLanded));
    expect(
      [
        for (final e in events)
          switch (e) {
            WorkerWorking(:final threads) => 'working $threads',
            WorkerProgress(:final threads, :final progress) =>
              '$progress $threads',
            WorkerLanded(:final version) => 'landed $version',
            _ => '$e',
          },
      ],
      [
        'working [${item.threadId}]',
        'PartialEdit(index.html, 2 chars) [${item.threadId}]',
        'OutputTokens(30) [${item.threadId}]',
        'landed 2',
      ],
    );
    expect(events.every((e) => e.artifactId == aid), isTrue);
  });

  test('batches of two artifacts run one after the other', () async {
    final a = api.seedArtifact('A', {'index.html': page});
    final b = api.seedArtifact('B', {'index.html': page});
    workerWith([
      change(edits: [edit('Save', 'A')]),
      change(edits: [edit('Save', 'B')]),
    ]).start();
    await idle();
    backend.gate = Completer<void>();
    api.sendComment(a, 'x');
    await backend.arrived.future;
    api.sendComment(b, 'y');
    await until(() => api.waitingPolls == 1);
    expect(backend.seen, hasLength(1));
    backend.gate!.complete();
    backend.gate = null;
    await idle();
    expect(api.filesOf(a, 2)['index.html'], contains('>A<'));
    expect(api.filesOf(b, 2)['index.html'], contains('>B<'));
    expect([for (final e in events) e.artifactId], [a, a, b, b]);
  });

  test(
    'one poll with comments on two artifacts makes one change for each',
    () async {
      final a = api.seedArtifact('A', {'index.html': page});
      final b = api.seedArtifact('B', {'index.html': page});
      workerWith([
        change(edits: [edit('Save', 'A')]),
        change(edits: [edit('Save', 'B')]),
      ]);
      final a1 = api.sendComment(a, 'one', queue: false);
      final b1 = api.sendComment(b, 'two', queue: false);
      final a2 = api.sendComment(a, 'three', queue: false);
      worker.start();
      await idle();
      api.pushFeedback([a1, b1, a2]);
      await until(() => api.currentVersion(b) == 2);
      await idle();
      expect(
        backend.seen[0].userText,
        allOf(contains('one'), contains('three')),
      );
      expect(backend.seen[1].userText, contains('two'));
    },
  );

  test('unknown_session re-registers and polls again', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(edits: [edit('Save', 'Keep')]),
    ]).start();
    await idle();
    final first = api.sessionId;
    api.endSession('install');
    api.releasePolls();
    await until(() => registrations == 2 && api.waitingPolls == 1);
    final second = api.sessionId;
    expect(second, isNot(first));
    final polls = [
      for (final c in api.calls)
        if (c.name == 'pollFeedback') c.args['sid'],
    ];
    expect(polls.last, second);

    final item = api.sendComment(aid, 'Say Keep');
    await until(() => api.acked.isNotEmpty);
    expect(api.acked, [item.commentId]);
    final ack = api.calls.lastWhere((c) => c.name == 'ackFeedback');
    expect(ack.args['sid'], second);
  });

  test('a failing poll backs off, then registers again', () async {
    workerWith([]);
    api
      ..failNext('pollFeedback', unreachable)
      ..failNext('pollFeedback', unreachable);
    worker.start();
    await until(() => api.waitingPolls == 1);
    expect(registrations, 3);
    expect(delays, hasLength(2));
    expect(delays[1], greaterThan(delays[0]));
  });

  test('worker_survives_daemon_restart_mid_change', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
    ]).start();
    await idle();
    backend.gate = Completer<void>();
    final item = api.sendComment(aid, 'Say Keep');
    await backend.arrived.future;

    // The daemon restarts while the backend runs: the long poll breaks and
    // the old session is gone. The worker registers again and recovers,
    // leaving the change in flight alone.
    api.endSession('install');
    api.releasePolls();
    await until(() => registrations == 2 && api.waitingPolls == 1);
    expect(backend.seen, hasLength(1), reason: 'the change in flight is kept');

    // The change in flight then gets no answer to its publish.
    api.failNext('publish', unreachable);
    backend.gate!.complete();
    backend.gate = null;
    await until(() => api.currentVersion(aid) == 2);
    await idle();

    expect(backend.seen, hasLength(2), reason: 'run again exactly once');
    expect(publishedOn(aid), ['1', '1']);
    expect(api.currentVersion(aid), 2, reason: 'exactly one new version');
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.authorKind], ['viewer', 'agent']);
    expect(t.comments.last.body, 'Keep.');
    expect(t.status, 'resolved');
  });

  test(
    'recovery while a change runs leaves its threads to that change',
    () async {
      final aid = api.seedArtifact('Runs', {'index.html': page});
      workerWith([
        change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      ]).start();
      await idle();
      backend.gate = Completer<void>();
      final item = api.sendComment(aid, 'Say Keep');
      await backend.arrived.future;

      // The worker registers again while the change runs: recovery lists the
      // thread as stranded, and the running change already has it.
      api.endSession('install');
      api.releasePolls();
      await until(() => registrations == 2 && api.waitingPolls == 1);
      expect(
        api.calls.where((c) => c.name == 'allThreads'),
        hasLength(2),
        reason: 'recovered on start and after the registration',
      );
      expect(
        worker.isIdle,
        isFalse,
        reason: 'the change in flight is still running',
      );

      backend.gate!.complete();
      backend.gate = null;
      await until(() => api.currentVersion(aid) == 2);
      await idle();
      // Nothing was queued for the thread: once the change has replied, its
      // threads are not listed again for a recheck.
      final names = [for (final c in api.calls) c.name];
      expect(names, contains('replyAsAgent'));
      expect(
        names.skip(names.lastIndexOf('replyAsAgent')),
        isNot(contains('threads')),
      );
      expect(backend.seen, hasLength(1));
      expect(api.threadOf(aid, item.threadId).comments.last.body, 'Keep.');
    },
  );

  test('a change that fails before the poll sees the restart is recovered '
      'once the worker registers again', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
    ]).start();
    await idle();
    const gone = ClaxApiError(
      status: 400,
      code: 'unknown_session',
      message: 'the session has ended',
    );
    api
      ..failNext('publish', gone)
      ..failNext('replyAsAgent', gone);
    final item = api.sendComment(aid, 'Say Keep');
    await until(() => worker.isIdle && backend.seen.length == 1);
    expect(api.currentVersion(aid), 1);

    api.endSession('install');
    api.releasePolls();
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(backend.seen, hasLength(2));
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.authorKind], ['viewer', 'agent']);
  });

  test('recovery on start re-runs only acknowledged, unanswered threads, '
      'and never applies a change twice', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    final sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
    Future<FeedbackItem> taken(String body) async {
      final item = api.sendComment(aid, body);
      final got = await api.pollFeedback(sid);
      expect(got.items.single.threadId, item.threadId);
      return item;
    }

    // Taken by a poll, then the app died before any reply: stranded.
    final stranded = await taken('Say Keep');
    // Taken and answered: done.
    final answered = await taken('Hello');
    await api.replyAsAgent(aid, answered.threadId, 'Hi.');
    // Taken, published, and the app died before the reply: landed.
    final landed = await taken('Add a footer');
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        files: {'index.html': '$page<footer>'},
        addresses: [landed.threadId],
      ),
    );
    // Sent and not yet taken: the poll delivers it.
    final pending = api.sendComment(aid, 'Make it red', queue: false);

    workerWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Red.', edits: [edit('<button>', '<button class=red>')]),
    ]).start();
    await idle();

    expect(backend.seen, hasLength(1));
    expect(backend.seen.single.userText, contains('Say Keep'));
    expect(api.threadOf(aid, stranded.threadId).comments.last.body, 'Keep.');
    expect(
      api.threadOf(aid, landed.threadId).comments.last.body,
      'Made this change in version 2.',
    );
    expect(api.threadOf(aid, answered.threadId).comments, hasLength(2));
    expect(api.threadOf(aid, pending.threadId).comments, hasLength(1));
    expect(api.currentVersion(aid), 3);

    api.pushFeedback([pending]);
    await until(() => api.currentVersion(aid) == 4);
    await idle();
    expect(backend.seen, hasLength(2), reason: 'the pending one runs once');
    expect(api.threadOf(aid, pending.threadId).comments.last.body, 'Red.');
    expect(
      api.filesOf(aid, 4)['index.html'],
      allOf(contains('<button class=red>Keep</button>'), contains('<footer>')),
    );
  });

  test('stop cancels the change in flight without a reply', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(edits: [edit('Save', 'Keep')]),
    ]).start();
    await idle();
    backend.gate = Completer<void>();
    final item = api.sendComment(aid, 'Say Keep');
    await backend.arrived.future;
    await worker.stop();
    expect(api.currentVersion(aid), 1);
    expect(api.threadOf(aid, item.threadId).comments, hasLength(1));
    expect(api.callNames, isNot(contains('replyAsAgent')));
    expect(api.working, isEmpty);
    await until(() => events.last is WorkerCancelled);
  });

  test('comments queued while a change runs join one run', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Done.', edits: [edit('<button>', '<button class=big>')]),
    ]).start();
    await idle();
    backend.gate = Completer<void>();
    api.sendComment(aid, 'Say Keep');
    await backend.arrived.future;
    final later = <FeedbackItem>[];
    for (final body in ['Bigger', 'Bolder', 'Brighter']) {
      later.add(api.sendComment(aid, body));
      await until(() => api.waitingPolls == 1, reason: '$body taken');
    }
    backend.gate!.complete();
    backend.gate = null;
    await idle();

    expect(backend.seen, hasLength(2), reason: 'one run for the three');
    expect(
      backend.seen[1].userText,
      allOf(contains('Bigger'), contains('Bolder'), contains('Brighter')),
    );
    expect(publishedOn(aid), ['1', '2']);
    for (final i in later) {
      expect(api.threadOf(aid, i.threadId).comments.last.body, 'Done.');
    }
  });

  test('an interrupted change is run again once the daemon answers, '
      'without a registration', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
    ]).start();
    await idle();
    api.failNext('publish', unreachable);
    final item = api.sendComment(aid, 'Say Keep');
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(registrations, 1);
    expect(backend.seen, hasLength(2));
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.body], ['Say Keep', 'Keep.']);
    expect(events.whereType<WorkerInterrupted>(), hasLength(1));
  });

  test('an interrupted change is run again at most once before the next '
      'registration', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(edits: [edit('Save', 'Keep')]),
      change(edits: [edit('Save', 'Keep')]),
    ]).start();
    await idle();
    api
      ..failNext('publish', unreachable)
      ..failNext('publish', unreachable);
    api.sendComment(aid, 'Say Keep');
    await until(() => events.whereType<WorkerInterrupted>().length == 2);
    await idle();
    expect(backend.seen, hasLength(2));
    expect(api.currentVersion(aid), 1);
  });

  test(
    'a version that answered an earlier comment is not taken as the '
    'answer to a newer one, and the earlier comment is not run again',
    () async {
      final commenting = CommentingApi();
      api = commenting;
      final aid = api.seedArtifact('Runs', {'index.html': page});
      workerWith([
        change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
        change(reply: 'Red.', edits: [edit('<button>', '<button class=red>')]),
        change(reply: 'Red.', edits: [edit('<button>', '<button class=red>')]),
      ]).start();
      await idle();

      // c1 runs; c2 arrives on the same thread while it does.
      final gate1 = backend.gate = Completer<void>();
      final c1 = api.sendComment(aid, 'Say Keep');
      await backend.arrived.future;
      final c2 = commenting.addComment(aid, c1.threadId, 'Make it red');
      api.pushFeedback([c2]);
      await until(() => api.waitingPolls == 1, reason: 'c2 taken');

      // c1's change publishes; c2's run is then interrupted.
      backend
        ..arrived = Completer<void>()
        ..gate = Completer<void>();
      gate1.complete();
      await backend.arrived.future;
      expect(api.currentVersion(aid), 2);
      expect(api.versionOf(aid, 2).addresses, isEmpty);
      api.failNext('publish', unreachable);
      backend.gate!.complete();
      backend.gate = null;

      // Recovery runs c2 alone, and its reply is c2's.
      await until(() => api.currentVersion(aid) == 3);
      await idle();
      expect(backend.seen, hasLength(3));
      expect(backend.seen[2].userText, contains('"Make it red"'));
      expect(backend.seen[2].userText, isNot(contains('"Say Keep"')));
      final t = api.threadOf(aid, c1.threadId);
      expect(
        [for (final c in t.comments) c.body],
        ['Say Keep', 'Make it red', 'Red.'],
      );
      expect(
        api.filesOf(aid, 3)['index.html'],
        '<!doctype html><title>Runs</title><button class=red>Keep</button>',
      );
    },
  );

  test('a publish whose answer was lost has its documents written and its '
      'reply posted by recovery', () async {
    final lossy = LostAnswerApi();
    api = lossy;
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(
        reply: 'Keep.',
        edits: [edit('Save', 'Keep')],
        db: const [
          DbOp(op: 'set', collection: 'runs', docId: 'a', data: {'km': 1}),
        ],
      ),
    ]).start();
    await idle();
    lossy.loseNextPublish = true;
    final item = api.sendComment(aid, 'Say Keep');
    await until(
      () => api.threadOf(aid, item.threadId).comments.length == 2,
      reason: 'the reply',
    );
    await idle();
    expect(backend.seen, hasLength(1));
    expect(api.currentVersion(aid), 2);
    expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 1});
    final t = api.threadOf(aid, item.threadId);
    expect(t.comments.last.body, 'Keep.');
    expect(t.status, 'resolved');
  });

  test('a document batch without an answer is finished by recovery, not '
      'undone', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(
        reply: 'Keep.',
        edits: [edit('Save', 'Keep')],
        db: const [
          DbOp(op: 'set', collection: 'runs', docId: 'a', data: {'km': 1}),
        ],
      ),
    ]).start();
    await idle();
    api.failNext('batchDocs', unreachable);
    final item = api.sendComment(aid, 'Say Keep');
    await until(
      () => api.threadOf(aid, item.threadId).comments.length == 2,
      reason: 'the reply',
    );
    await idle();
    expect(api.callNames, isNot(contains('restoreVersion')));
    expect(api.currentVersion(aid), 2);
    expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 1});
    expect(api.threadOf(aid, item.threadId).comments.last.body, 'Keep.');
    expect(events.whereType<WorkerInterrupted>(), hasLength(1));
  });

  test('a batch that ran but lost its answer is found written by its '
      'content', () async {
    final lossy = LostAnswerApi();
    api = lossy;
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(
        reply: 'Keep.',
        edits: [edit('Save', 'Keep')],
        db: const [
          DbOp(op: 'set', collection: 'runs', docId: 'a', data: {'km': 1}),
        ],
      ),
    ]).start();
    await idle();
    lossy.loseNextBatch = true;
    final item = api.sendComment(aid, 'Say Keep');
    await until(
      () => api.threadOf(aid, item.threadId).comments.length == 2,
      reason: 'the reply',
    );
    await idle();
    expect(api.callNames, isNot(contains('restoreVersion')));
    expect(api.threadOf(aid, item.threadId).comments.last.body, 'Keep.');
    expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 1});
  });

  test('a batch whose pin conflicts with other content fails the change '
      'with the thread open', () async {
    final lossy = LostAnswerApi();
    api = lossy;
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(
        reply: 'Keep.',
        edits: [edit('Save', 'Keep')],
        db: const [
          DbOp(op: 'set', collection: 'runs', docId: 'a', data: {'km': 1}),
        ],
      ),
    ]).start();
    await idle();
    // The batch never runs; the reloaded page then creates the document
    // with its own defaults.
    lossy.pageWritesBeforeBatch = ('runs/a', {'km': 9});
    final item = api.sendComment(aid, 'Say Keep');
    await until(
      () => api.threadOf(aid, item.threadId).comments.length == 2,
      reason: 'the reply',
    );
    await idle();
    final t = api.threadOf(aid, item.threadId);
    expect(t.comments.last.body, startsWith("Couldn't make that change: "));
    expect(t.status, 'open');
    expect(backend.seen, hasLength(1));
    final restore = api.calls.singleWhere((c) => c.name == 'restoreVersion');
    expect(restore.args, {'aid': aid, 'n': 1, 'ifVersion': 2});
    expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 9});
  });

  test('a lost publish whose documents are then refused is undone and run '
      'again', () async {
    final lossy = LostAnswerApi();
    api = lossy;
    final aid = api.seedArtifact('Runs', {'index.html': page});
    const missing = DbOp(
      op: 'update',
      collection: 'runs',
      docId: 'gone',
      data: {'km': 1},
    );
    workerWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')], db: [missing]),
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
    ]).start();
    await idle();
    lossy.loseNextPublish = true;
    final item = api.sendComment(aid, 'Say Keep');
    await until(() => api.currentVersion(aid) == 4);
    await idle();
    final restore = api.calls.singleWhere((c) => c.name == 'restoreVersion');
    expect(restore.args, {'aid': aid, 'n': 1, 'ifVersion': 2});
    expect(backend.seen, hasLength(2));
    expect(api.filesOf(aid, 4)['index.html'], contains('Keep'));
    expect(api.threadOf(aid, item.threadId).comments.last.body, 'Keep.');
  });

  test('a recovered thread answered before its run is not run again', () async {
    final stale = StaleThreadsApi();
    api = stale;
    final aid = api.seedArtifact('Runs', {'index.html': page});
    final sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
    final item = api.sendComment(aid, 'Say Keep');
    await api.pollFeedback(sid);
    // The snapshot is taken while the thread is stranded; its change then
    // lands and is answered before the recovered job runs.
    stale.snapshot = await api.allThreads();
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        files: {'index.html': '$page<p>'},
        addresses: [item.threadId],
      ),
    );
    await api.replyAsAgent(aid, item.threadId, 'Keep.');
    workerWith([]).start();
    await idle();
    expect(backend.seen, isEmpty);
    expect(api.threadOf(aid, item.threadId).comments, hasLength(2));
  });

  test('a failed change is reported with its reason', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    workerWith([
      change(removed: ['x']),
      change(removed: ['x']),
    ]).start();
    await idle();
    api.sendComment(aid, 'x');
    await until(() => events.whereType<WorkerFailed>().isNotEmpty);
    expect(
      events.whereType<WorkerFailed>().single.reason,
      contains('removed names "x"'),
    );
  });
}

/// A fake whose all-threads listing answers with [snapshot] once, as a
/// listing taken just before the threads changed.
class StaleThreadsApi extends FakeClaxApi {
  List<ArtifactThreads>? snapshot;

  @override
  Future<List<ArtifactThreads>> allThreads({
    bool includeResolved = false,
  }) async {
    final live = await super.allThreads(includeResolved: includeResolved);
    final s = snapshot;
    snapshot = null;
    return s ?? live;
  }
}

/// A fake that can add a viewer comment to an existing thread, as the
/// person writing again on it. The comment shows in every thread view,
/// after the comment it follows.
class CommentingApi extends FakeClaxApi {
  CommentingApi() : super(clock: _tick);

  static var _t = 0;
  static String _tick() =>
      DateTime.utc(2026, 10, 12).add(Duration(seconds: ++_t)).toIso8601String();

  final Map<String, List<(String after, ThreadComment comment)>> _extra = {};
  var _n = 0;

  FeedbackItem addComment(String aid, String tid, String body) {
    final before = super.threadOf(aid, tid).comments.last.id;
    final c = ThreadComment(
      id: 'added-${++_n}',
      authorKind: 'viewer',
      authorName: 'Alex',
      body: body,
      createdAt: _tick(),
    );
    _extra.putIfAbsent(tid, () => []).add((before, c));
    return FeedbackItem(
      feedbackId: 'added-feedback-$_n',
      threadId: tid,
      commentId: c.id,
      artifactId: aid,
      body: body,
      author: 'Alex',
      version: currentVersion(aid),
    );
  }

  ThreadView _with(ThreadView t) {
    final extra = _extra[t.id];
    if (extra == null) return t;
    final comments = List.of(t.comments);
    for (final (after, c) in extra) {
      comments.insert(comments.indexWhere((x) => x.id == after) + 1, c);
    }
    return ThreadView(
      id: t.id,
      artifactId: t.artifactId,
      versionN: t.versionN,
      status: t.status,
      sentToAgent: t.sentToAgent,
      comments: comments,
      anchor: t.anchor,
      hasClip: t.hasClip,
      createdAt: t.createdAt,
      resolvedAt: t.resolvedAt,
      resolvedBy: t.resolvedBy,
      feedbackState: t.feedbackState,
      addressedIn: t.addressedIn,
    );
  }

  @override
  ThreadView threadOf(String aid, String tid) =>
      _with(super.threadOf(aid, tid));

  @override
  Future<List<ThreadView>> threads(String aid) async => [
    for (final t in await super.threads(aid)) _with(t),
  ];

  @override
  Future<List<ArtifactThreads>> allThreads({
    bool includeResolved = false,
  }) async => [
    for (final a in await super.allThreads(includeResolved: includeResolved))
      ArtifactThreads(
        artifactId: a.artifactId,
        title: a.title,
        currentVersion: a.currentVersion,
        files: a.files,
        threads: [for (final t in a.threads) _with(t)],
      ),
  ];
}

/// A fake whose next publish or document batch can lose its answer.
class LostAnswerApi extends FakeClaxApi {
  var loseNextPublish = false;

  /// The next document batch runs but answers as if the daemon were gone.
  var loseNextBatch = false;

  /// Before the next document batch, the page writes this document, and
  /// the batch answers as if the daemon were gone without running.
  (String, Map<String, Object?>)? pageWritesBeforeBatch;

  @override
  Future<void> batchDocs(String aid, List<DocWrite> writes) async {
    if (pageWritesBeforeBatch case (final path, final data)) {
      pageWritesBeforeBatch = null;
      putDoc(aid, path, data);
      throw unreachable;
    }
    await super.batchDocs(aid, writes);
    if (loseNextBatch) {
      loseNextBatch = false;
      throw unreachable;
    }
  }

  @override
  Future<PublishResult> publish(String aid, PublishRequest r) async {
    final res = await super.publish(aid, r);
    if (loseNextPublish) {
      loseNextPublish = false;
      throw unreachable;
    }
    return res;
  }
}
