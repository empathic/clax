import 'dart:async';

import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/read_consent.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const page = '<!doctype html><title>Runs</title><button>Save</button>';

const unreachable = ClaxApiError(
  status: 0,
  code: 'daemon_unreachable',
  message: 'no connection',
);

/// A question the agent asks with Clax's `ask`.
const pick = {
  'questions': [
    {
      'question': 'Kilometres or miles?',
      'header': 'Units',
      'options': [
        {'label': 'km'},
        {'label': 'mi'},
      ],
    },
  ],
  'question_id': null,
  'timeout_s': null,
  'cancel': null,
};

/// The tool loop's [AgentLoop] (the single call is in
/// loop_single_call_test.dart): a [HeldRunner] over the real Toolbox, the
/// fake daemon and a fake inspector.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late HeldRunner runner;
  late FakeInspector inspector;
  late RecordingTimings timings;
  late MemoryChangeLedger ledger;
  late MemoryAskedQuestions asked;
  late String sid;

  AgentLoop loopWith(
    List<List<ScriptedCall>> runs, {
    bool images = true,
    AgentReadConsent? consent,
  }) {
    runner = HeldRunner(runs, sendsImages: images);
    return AgentLoop(
      api: api,
      runner: () => runner,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: timings,
      inspector: inspector,
      baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
      ledger: ledger,
      asked: asked,
      readConsent: consent,
      clock: StepClock(),
    );
  }

  List<String> bodies(String aid, String tid) => [
    for (final c in api.threadOf(aid, tid).comments) c.body,
  ];

  setUp(() async {
    api = FakeClaxApi();
    inspector = FakeInspector();
    timings = RecordingTimings();
    ledger = MemoryChangeLedger();
    asked = MemoryAskedQuestions();
    sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
  });

  group('newApp', () {
    test('writes, renders and finishes: the app is created as the session, '
        'and its draft deleted', () async {
      final loop = loopWith([
        newAppCalls(capabilities: const {'db': {}}),
      ]);
      final aid = await loop.newApp('a run tracker', session: sid);

      final created = api.calls.singleWhere((c) => c.name == 'createArtifact');
      expect(created.args['session'], sid);
      final r = created.args['request']! as PublishRequest;
      expect(r.title, 'Runs');
      expect(r.note, 'Made a run tracker.');
      expect(r.capabilities, {'db': <String, Object?>{}});
      expect(api.filesOf(aid, 1)['index.html'], contains('<h1>Runs</h1>'));
      expect(api.callNames, contains('createDraft'));
      expect(await api.listDrafts(), isEmpty);
      expect(inspector.calls, hasLength(1));
    });

    test('runs on the tool loop prompt: the tool system prompt and the '
        'first message of a new app', () async {
      final loop = loopWith([newAppCalls()]);
      await loop.newApp('a run tracker', session: sid);
      final input = runner.seen.single;
      final system = await PromptBuilder(rootBundle).toolSystem();
      expect(input.system, system);
      expect(input.system, isNot(await PromptBuilder(rootBundle).system()));
      expect(
        input.first,
        PromptBuilder(rootBundle).firstNew('a run tracker', const []),
      );
    });

    test('forwards the run\'s progress from attempt 1: narration, tools, '
        'the streamed page and the inspection', () async {
      final loop = loopWith([newAppCalls()]);
      final events = <AgentProgress>[];
      await loop.newApp('x', session: sid, onProgress: events.add);
      expect(
        events.first,
        isA<AttemptStarted>().having((e) => e.attempt, 'attempt', 1),
      );
      expect(
        [
          for (final e in events)
            switch (e) {
              Narration(:final text) => 'narration $text',
              ToolStarted(:final name) => 'tool $name',
              PartialFile(:final path) => 'partial $path',
              Inspected(:final errors) => 'inspected $errors',
              AttemptStarted(:final attempt) => 'attempt $attempt',
              _ => '$e',
            },
        ],
        [
          'attempt 1',
          'narration Writing the tracker',
          'tool set_meta',
          'tool write_file',
          'partial index.html',
          'tool render',
          'inspected 0',
          'tool finish',
        ],
      );
    });

    test('a progress listener that throws does not stop the run', () async {
      final loop = loopWith([newAppCalls()]);
      final aid = await loop.newApp(
        'x',
        session: sid,
        onProgress: (_) => throw StateError('listener'),
      );
      expect(api.currentVersion(aid), 1);
    });

    test('other apps go in by ID and title only, and none of their documents '
        'is read', () async {
      final other = api.seedArtifact(
        'Shopping',
        {'index.html': page},
        capabilities: const {'db': {}},
      );
      api.putDoc(other, 'items/milk', {'name': 'milk'});
      final loop = loopWith([newAppCalls()]);
      await loop.newApp('x', session: sid);
      expect(runner.text(0), contains('{"id":"$other","title":"Shopping"}'));
      expect(runner.text(0), isNot(contains('milk')));
      expect([
        for (final c in api.calls)
          if (c.args['aid'] == other) c.name,
      ], isEmpty);
    });

    test(
      'a run that gives up throws its reason and deletes its draft',
      () async {
        final loop = loopWith([
          [
            ...newAppCalls().take(3),
            const ScriptedCall('give_up', {'reason': 'it needs a camera'}),
          ],
        ]);
        await expectLater(
          loop.newApp('x', session: sid),
          throwsA(
            isA<AgentError>().having(
              (e) => e.message,
              'message',
              'it needs a camera',
            ),
          ),
        );
        expect(api.callNames, isNot(contains('createArtifact')));
        expect(await api.listDrafts(), isEmpty);
      },
    );

    test('a refusal the runner ends the run on throws, and deletes the draft '
        'its earlier render made', () async {
      final loop = loopWith([
        [
          ...newAppCalls().take(3),
          const ScriptedCall.end(
            RunGaveUp('The model declined: cyber', answered: false),
          ),
        ],
      ]);
      await expectLater(
        loop.newApp('x', session: sid),
        throwsA(
          isA<AgentError>().having(
            (e) => e.message,
            'message',
            'The model declined: cyber',
          ),
        ),
      );
      expect(api.callNames, contains('createDraft'));
      expect(await api.listDrafts(), isEmpty);
    });

    test('a model failure is thrown as it came, never resent, and the draft '
        'is deleted', () async {
      const busy = AgentError('overloaded', retryable: true);
      final loop = loopWith([
        [...newAppCalls().take(3), const ScriptedCall.fail(busy)],
      ]);
      await expectLater(loop.newApp('x', session: sid), throwsA(same(busy)));
      expect(runner.seen, hasLength(1));
      expect(await api.listDrafts(), isEmpty);
    });

    test('a cancelled new app creates nothing and deletes its draft', () async {
      final cancel = Completer<void>();
      final loop = loopWith([newAppCalls()]);
      runner.gate = Completer<void>();
      final made = loop.newApp('x', session: sid, cancelled: cancel.future);
      await runner.arrived.future;
      cancel.complete();
      await expectLater(
        made,
        throwsA(
          isA<AgentError>().having(
            (e) => e.kind,
            'kind',
            AgentErrorKind.cancelled,
          ),
        ),
      );
      expect(api.callNames, isNot(contains('createArtifact')));
    });

    test('a create whose answer was lost after it landed returns the app it '
        'made', () async {
      final lossy = _LostCreateApi();
      api = lossy;
      sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
      final before = api.seedArtifact('Older', {'index.html': page});
      lossy.loseNextCreate = true;
      final loop = loopWith([newAppCalls()]);
      final aid = await loop.newApp('x', session: sid);
      expect(aid, isNot(before));
      expect(api.currentVersion(aid), 1);
      expect(
        api.calls.where((c) => c.name == 'createArtifact'),
        hasLength(1),
        reason: 'found, not made again',
      );
    });

    test('a create that got no answer and did not land says to look for the '
        'app before trying again', () async {
      api.failNext('createArtifact', unreachable);
      final loop = loopWith([newAppCalls()]);
      await expectLater(
        loop.newApp('x', session: sid),
        throwsA(
          isA<AgentError>().having(
            (e) => e.message,
            'message',
            contains('Look for it in your apps'),
          ),
        ),
      );
    });

    test('a question still open when the new app ends is withdrawn', () async {
      final loop = loopWith([
        [const ScriptedCall('ask', pick), ...newAppCalls()],
      ]);
      await loop.newApp('x', session: sid);
      final q = (await api.questions(status: 'all')).single;
      expect(q.status, 'withdrawn');
      expect(await asked.all(), isEmpty);
    });

    test('the run is timed with its model, runner, turns, tokens, cost and '
        'outcome', () async {
      final loop = loopWith([newAppCalls()]);
      await loop.newApp('x', session: sid);
      final r = timings.stopped.single;
      expect(r.name, agentNewTiming);
      expect(r.extra, {
        'model': 'claude-haiku-5-5',
        'backend': 'scripted',
        'turns': 4,
        'renders': 1,
        'input_tokens': 0,
        'cache_read_tokens': 0,
        'cache_write_tokens': 0,
        'output_tokens': 0,
        'cost_usd': 0.0,
        'outcome': 'finished',
      });
    });
  });

  group('change', () {
    late String aid;

    setUp(() {
      aid = api.seedArtifact(
        'Runs',
        {'index.html': page},
        capabilities: const {'db': {}},
      );
    });

    test('str_replace, render, finish: the version lands in place, the '
        'thread is answered and resolved, the comment acknowledged', () async {
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final loop = loopWith([
        changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]),
      ]);
      final out = await loop.change(aid, [item], session: sid);

      expect(out, isA<ChangeLanded>().having((o) => o.version, 'version', 2));
      expect(out.answered, isTrue);
      expect(
        api.filesOf(aid, 2)['index.html'],
        contains('<button>Keep</button>'),
      );
      final publish = api.calls.singleWhere((c) => c.name == 'publish');
      final r = publish.args['request']! as PublishRequest;
      expect(r.ifVersion, 1);
      expect(r.addresses, [item.threadId]);
      expect(bodies(aid, item.threadId), ['Say Keep', 'Keep.']);
      expect(api.threadOf(aid, item.threadId).status, 'resolved');
      expect(api.acked, [item.commentId]);
      expect(api.working, isEmpty);
      expect(await api.listDrafts(), isEmpty);
      expect(await ledger.of(aid), hasLength(1));
    });

    test('the first message is the tool loop\'s, with the files, the '
        'comments and the late answers', () async {
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final loop = loopWith([changeCalls()]);
      await loop.change(
        aid,
        [item],
        session: sid,
        answers: '[clax] The person answered your question "Units" (q1):',
      );
      final input = runner.seen.single;
      expect(input.system, await PromptBuilder(rootBundle).toolSystem());
      final text = runner.text(0);
      expect(text, contains('<file path="index.html">'));
      expect(text, contains('"Say Keep"'));
      expect(text, contains(r'The person answered your question \"Units\"'));
    });

    test('a finish with nothing changed answers without a version', () async {
      final item = api.sendComment(aid, 'What does Save do?', queue: false);
      final loop = loopWith([changeCalls(reply: 'It saves.', resolve: false)]);
      final out = await loop.change(aid, [item], session: sid);
      expect(out, isA<ChangeAnswered>());
      expect(api.currentVersion(aid), 1);
      expect(bodies(aid, item.threadId).last, 'It saves.');
    });

    test(
      'give_up replies once with the reason and leaves the thread open',
      () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final loop = loopWith([
          [
            const ScriptedCall('give_up', {'reason': 'it needs a camera'}),
          ],
        ]);
        final out = await loop.change(aid, [item], session: sid);
        expect(
          out,
          isA<ChangeFailed>()
              .having((o) => o.reason, 'reason', 'it needs a camera')
              .having((o) => o.answered, 'answered', isTrue),
        );
        expect(bodies(aid, item.threadId), [
          'x',
          failureReply('it needs a camera'),
        ]);
        expect(api.threadOf(aid, item.threadId).status, 'open');
      },
    );

    test('a refusal the runner ends the run on gets the failure reply, the '
        'comment acknowledged and the draft deleted', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([
        [
          ...changeCalls(edits: [('Save', 'Keep')]).take(2),
          const ScriptedCall.end(
            RunGaveUp('The model declined: cyber', answered: false),
          ),
        ],
      ]);
      final out = await loop.change(aid, [item], session: sid);
      expect(
        out,
        isA<ChangeFailed>()
            .having((o) => o.reason, 'reason', 'The model declined: cyber')
            .having((o) => o.answered, 'answered', isTrue),
      );
      expect(bodies(aid, item.threadId), [
        'x',
        "Couldn't make that change: The model declined: cyber",
      ]);
      expect(api.acked, [item.commentId]);
      expect(api.callNames, contains('createDraft'));
      expect(await api.listDrafts(), isEmpty);
      expect(api.currentVersion(aid), 1);
      expect(api.working, isEmpty);
    });

    test('a model failure gets the failure reply with its message', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([
        [const ScriptedCall.fail(AgentError('overloaded', retryable: true))],
      ]);
      final out = await loop.change(aid, [item], session: sid);
      expect(out, isA<ChangeFailed>());
      expect(bodies(aid, item.threadId).last, failureReply('overloaded'));
      expect(runner.seen, hasLength(1), reason: 'never resent');
    });

    test(
      'a runner-made ending skips a thread the person wrote on again',
      () async {
        final commenting = _NewerApi();
        api = commenting;
        sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
        aid = api.seedArtifact('Runs', {'index.html': page});
        final item = api.sendComment(aid, 'x', queue: false);
        commenting.newerOn = item.threadId;
        final loop = loopWith([
          [
            const ScriptedCall.end(
              RunGaveUp('The model declined: cyber', answered: false),
            ),
          ],
        ]);
        await loop.change(aid, [item], session: sid);
        expect(api.callNames, isNot(contains('replyAsAgent')));
      },
    );

    test(
      'a cap with no clean draft replies once and publishes nothing',
      () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final loop = loopWith([
          [
            for (var i = 0; i < 7; i++)
              const ScriptedCall('list_files', {}, 'Looking'),
          ],
        ]);
        final out = await loop.change(aid, [item], session: sid);
        expect(out, isA<ChangeFailed>());
        expect(bodies(aid, item.threadId), [
          'x',
          failureReply('it ran out of time (6 turns).'),
        ]);
        expect(api.currentVersion(aid), 1);
      },
    );

    test('a cap after a clean render publishes that draft, saying it stopped '
        'early, and leaves the thread open', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([
        [
          ...changeCalls(
            edits: [('Save', 'Keep')],
            narration: 'Renaming the button',
          ).take(2),
          for (var i = 0; i < 5; i++) const ScriptedCall('list_files', {}),
        ],
      ]);
      final out = await loop.change(aid, [item], session: sid);
      expect(out, isA<ChangeLanded>().having((o) => o.version, 'version', 2));
      expect(
        bodies(aid, item.threadId).last,
        'Renaming the button. I stopped early (6 turns), so this may not be '
        'everything you asked.',
      );
      expect(api.threadOf(aid, item.threadId).status, 'open');
      expect(await api.listDrafts(), isEmpty);
    });

    test('a cancel ends the change with nothing published or replied, and '
        'the draft deleted', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      final cancel = Completer<void>();
      final loop = loopWith([
        changeCalls(edits: [('Save', 'Keep')]),
      ]);
      runner.gate = Completer<void>();
      final out = loop.change(
        aid,
        [item],
        session: sid,
        cancelled: cancel.future,
      );
      await runner.arrived.future;
      cancel.complete();
      expect(await out, isA<ChangeCancelled>());
      expect(api.callNames, isNot(contains('replyAsAgent')));
      expect(api.callNames, isNot(contains('publish')));
      expect(api.working, isEmpty);
    });

    test('a publish without an answer is interrupted: no reply, the ledger '
        'record kept', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      api.failNext('publish', unreachable);
      final loop = loopWith([
        changeCalls(edits: [('Save', 'Keep')]),
      ]);
      final out = await loop.change(aid, [item], session: sid);
      expect(out, isA<ChangeInterrupted>());
      expect(api.callNames, isNot(contains('replyAsAgent')));
      expect(await ledger.of(aid), hasLength(1));
    });

    test('a deleted app drops the change without a reply or a run', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      api.failNext(
        'readFiles',
        const ClaxApiError(status: 404, code: 'not_found', message: 'gone'),
      );
      final loop = loopWith([changeCalls()]);
      final out = await loop.change(aid, [item], session: sid);
      expect(out, isA<ChangeDropped>());
      expect(runner.seen, isEmpty);
      expect(api.callNames, isNot(contains('replyAsAgent')));
    });

    test('the documents shown are this app\'s own, at their versions; '
        'another app\'s are left out', () async {
      final mine = api.putDoc(aid, 'notes/n1', {'t': 'mine'});
      final other = api.seedArtifact(
        'Other',
        {'index.html': page},
        capabilities: const {'db': {}},
      );
      api.putDoc(other, 'notes/n1', {'t': 'theirs'});
      api.putDoc(other, 'notes/n1', {'t': 'theirs again'});
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([changeCalls()]);
      await loop.change(aid, [item], session: sid);
      expect(runner.states.single.ctx.shownDocs, {'notes/n1': mine.version});
      expect(runner.text(0), contains('"mine"'));
      expect(runner.text(0), isNot(contains('theirs')));
    });

    test('a shown document is written pinned to the version it was shown at, '
        'without a read', () async {
      api.putDoc(aid, 'runs/a', {'km': 1});
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([
        changeCalls(
          edits: [('Save', 'Keep')],
          db: const [
            DbOp(op: 'update', collection: 'runs', docId: 'a', data: {'km': 2}),
          ],
        ),
      ]);
      final out = await loop.change(aid, [item], session: sid);
      expect(out, isA<ChangeLanded>());
      expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 2});
    });

    test('the clip goes with a runner that sends images, as the first '
        'block', () async {
      final png = Uint8List.fromList([1, 2, 3]);
      final item = api.sendComment(aid, 'x', queue: false, clip: png);
      final loop = loopWith([changeCalls()]);
      await loop.change(aid, [item], session: sid);
      expect(runner.seen.single.first.first['type'], 'image');
    });

    test('no clip is read for a runner that sends no images', () async {
      final png = Uint8List.fromList([1, 2, 3]);
      final item = api.sendComment(aid, 'x', queue: false, clip: png);
      final loop = loopWith([changeCalls()], images: false);
      await loop.change(aid, [item], session: sid);
      expect(api.callNames, isNot(contains('clip')));
      expect([for (final b in runner.seen.single.first) b['type']], ['text']);
    });

    test('the read consent reaches the run', () async {
      final consent = FakeReadConsent();
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([changeCalls()], consent: consent);
      await loop.change(aid, [item], session: sid);
      expect(runner.states.single.readConsent, same(consent));
    });

    test('progress reaches onProgress, from attempt 1', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([
        changeCalls(edits: [('Save', 'Keep')], narration: 'Renaming'),
      ]);
      final events = <AgentProgress>[];
      await loop.change(aid, [item], session: sid, onProgress: events.add);
      expect(events.first, isA<AttemptStarted>());
      expect(events.whereType<Narration>().single.text, 'Renaming');
      expect(
        [for (final e in events.whereType<ToolStarted>()) e.name],
        ['str_replace', 'render', 'finish'],
      );
      expect(events.whereType<Inspected>(), hasLength(1));
    });

    test('the run is timed with the artifact and its outcome', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      final loop = loopWith([
        changeCalls(edits: [('Save', 'Keep')]),
      ]);
      await loop.change(aid, [item], session: sid);
      final r = timings.stopped.single;
      expect(r.name, agentChangeTiming);
      expect(r.extra['artifact'], aid);
      expect(r.extra['turns'], 3);
      expect(r.extra['outcome'], 'finished');
    });

    test('without an inspector a change is not run and says why', () async {
      final item = api.sendComment(aid, 'x', queue: false);
      runner = HeldRunner([changeCalls()]);
      final loop = AgentLoop(
        api: api,
        runner: () => runner,
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: timings,
        clock: StepClock(),
      );
      final out = await loop.change(aid, [item], session: sid);
      expect(out, isA<ChangeFailed>());
      expect(runner.seen, isEmpty);
      expect(bodies(aid, item.threadId).last, startsWith(failurePrefix));
    });

    group('questions', () {
      test('before a change runs, an earlier question asked for its '
          'comments is taken and withdrawn', () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final q = await api.askQuestion(sid, [
          {
            'question': 'Kilometres or miles?',
            'header': 'Units',
            'options': [
              {'label': 'km'},
              {'label': 'mi'},
            ],
          },
        ], artifactId: aid);
        await asked.put(
          AskedEntry(
            questionId: q.question.id,
            artifactId: aid,
            threadIds: [item.threadId],
            commentIds: [item.commentId],
          ),
        );
        final unrelated = AskedEntry(
          questionId: 'question-other',
          artifactId: aid,
          threadIds: const ['t-other'],
          commentIds: const ['c-other'],
        );
        await asked.put(unrelated);
        final loop = loopWith([changeCalls()]);
        await loop.change(aid, [item], session: sid);
        expect(
          (await api.questions(status: 'all'))
              .singleWhere((x) => x.id == q.question.id)
              .status,
          'withdrawn',
        );
        expect(
          [for (final e in await asked.all()) e.questionId],
          ['question-other'],
        );
        final names = api.callNames;
        expect(
          names.indexOf('withdrawQuestion'),
          lessThan(names.indexOf('setWorking')),
        );
      });

      test('a change that finishes with its question open is parked: the '
          'question and its entry stay', () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final loop = loopWith([
          [
            const ScriptedCall('ask', pick),
            ...changeCalls(reply: 'Which units? I asked you in Clax.'),
          ],
        ]);
        final out = await loop.change(aid, [item], session: sid);
        expect(out, isA<ChangeAnswered>());
        final q = (await api.questions()).single;
        expect(q.status, 'open');
        final e = (await asked.all()).single;
        expect(e.questionId, q.id);
        expect(e.commentIds, [item.commentId]);
      });

      test('a cancel that fires after the run ended leaves its parked '
          'question alone', () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final cancel = Completer<void>();
        final loop = loopWith([
          [
            const ScriptedCall('ask', pick),
            ...changeCalls(reply: 'I asked you in Clax.', resolve: false),
          ],
        ]);
        await loop.change(aid, [item], session: sid, cancelled: cancel.future);
        cancel.complete();
        await until(() => true);
        await Future<void>.delayed(Duration.zero);
        expect((await api.questions()).single.status, 'open');
        expect(await asked.all(), hasLength(1));
      });

      test('a change that gives up withdraws its open question and forgets '
          'it', () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final loop = loopWith([
          [
            const ScriptedCall('ask', pick),
            const ScriptedCall('give_up', {'reason': 'no answer'}),
          ],
        ]);
        await loop.change(aid, [item], session: sid);
        expect((await api.questions(status: 'all')).single.status, 'withdrawn');
        expect(await asked.all(), isEmpty);
      });

      test('a cap with no clean draft withdraws the open question', () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final loop = loopWith([
          [
            const ScriptedCall('ask', pick),
            for (var i = 0; i < 6; i++) const ScriptedCall('list_files', {}),
          ],
        ]);
        final out = await loop.change(aid, [item], session: sid);
        expect(out, isA<ChangeFailed>());
        expect((await api.questions(status: 'all')).single.status, 'withdrawn');
        expect(await asked.all(), isEmpty);
      });

      test('a runner-made ending withdraws the open question', () async {
        final item = api.sendComment(aid, 'x', queue: false);
        final loop = loopWith([
          [
            const ScriptedCall('ask', pick),
            const ScriptedCall.end(
              RunGaveUp('The model declined: x', answered: false),
            ),
          ],
        ]);
        await loop.change(aid, [item], session: sid);
        expect((await api.questions(status: 'all')).single.status, 'withdrawn');
      });
    });
  });
}

/// A fake whose next create lands but answers as if the daemon were gone.
class _LostCreateApi extends FakeClaxApi {
  var loseNextCreate = false;

  @override
  Future<PublishResult> createArtifact(PublishRequest r) async {
    final res = await super.createArtifact(r);
    if (loseNextCreate) {
      loseNextCreate = false;
      throw unreachable;
    }
    return res;
  }
}

/// A fake that shows a newer viewer comment on thread [newerOn].
class _NewerApi extends FakeClaxApi {
  String? newerOn;

  @override
  Future<List<ThreadView>> threads(String aid) async => [
    for (final t in await super.threads(aid))
      if (t.id == newerOn)
        ThreadView(
          id: t.id,
          artifactId: t.artifactId,
          versionN: t.versionN,
          status: t.status,
          sentToAgent: t.sentToAgent,
          feedbackState: t.feedbackState,
          comments: [
            ...t.comments,
            const ThreadComment(
              id: 'comment-newer',
              authorKind: 'viewer',
              authorName: 'Alex',
              body: 'And make it red',
              createdAt: '2026-10-12T00:00:01Z',
            ),
          ],
        )
      else
        t,
  ];
}
