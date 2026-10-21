import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart' show RunInput;
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/clax/transport.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/json_import.dart';
import 'package:clax_app/queue/agent_port.dart';
import 'package:clax_app/queue/codec.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/render.dart';
import 'package:clax_app/queue/runner.dart';
import 'package:clax_app/queue/worker_main.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../agent/tools/support.dart';
import '../support/fake_core.dart';
import '../support/fake_core_jobs.dart';
import '../support/paths.dart';

/// The effect runner over [FakeCore]'s job routes: here, and not in
/// `update`'s tests, the routes, the owner's header and the bodies are
/// checked against docs/contract.md ("Jobs"), and each answer against the
/// message it becomes.
void main() {
  late FakeCore core;
  late FakeJobs jobs;
  late _Agent agent;
  late _Render renders;
  late _Ui ui;
  late List<Msg> sent;
  late _Timers timers;
  late EffectRunner runner;
  var probe = true;

  EffectRunner runnerOn(
    FakeCore c,
    List<Msg> out, {
    AgentPort? agentPort,
    KillHook? kill,
    Future<ImportSummary?> Function(ClaxTransport core, String token)?
    importState,
  }) => EffectRunner(
    core: CoreTransport(
      c,
      timer: timers.make,
      retryDelay: (_) async {},
    ),
    agent: agentPort ?? agent,
    render: renders,
    ui: ui,
    probeClaude: () async => probe,
    session: () async => 's1',
    send: out.add,
    kill: kill,
    importState: importState,
  );

  setUp(() {
    core = FakeCore();
    jobs = FakeJobs(core);
    agent = _Agent();
    renders = _Render();
    ui = _Ui();
    sent = [];
    timers = _Timers();
    probe = true;
    runner = runnerOn(core, sent);
  });

  tearDown(() => runner.close());

  /// Starts the runner and claims the queue.
  Future<String> claimed() async {
    runner.start();
    await pumpEventQueue();
    runner.run(const ClaimOwner(0));
    await pumpEventQueue();
    return sent.whereType<OwnerClaimed>().single.token;
  }

  /// The requests to [path] (without the query), in order.
  List<CoreRequest> to(String method, String path) => [
    for (final r in core.requests)
      if (r.method == method && Uri.parse(r.path).path == path) r,
  ];

  String? ownerOf(CoreRequest r) => r.headers
      .where((h) => h.$1 == EffectRunner.ownerHeader)
      .map((h) => h.$2)
      .firstOrNull;

  Map<String, Object?> body(CoreRequest r) =>
      (FakeCore.bodyOf(r)! as Map).cast();

  T only<T extends Msg>() => sent.whereType<T>().single;

  group('the owner', () {
    test('a claim names the stream the core\'s ready named', () async {
      final token = await claimed();
      final claim = to('POST', '/api/jobs/owner').single;
      expect(body(claim), {'client': 'app', 'stream': 'fake-1'});
      expect(token, jobs.ownerToken);
      expect(only<OwnerClaimed>().effect, 0);
      expect(
        sent.whereType<OwnerChanged>().single.held,
        isTrue,
        reason: 'the claim subscribed the stream to jobs',
      );
    });

    test('a claim made before the ready waits for it', () async {
      runner.run(const ClaimOwner(3));
      await pumpEventQueue();
      expect(only<OwnerClaimed>().effect, 3);
    });

    test('two claimants: the second is owner_held, and hears the release', () async {
      await claimed();
      final second = <Msg>[];
      final other = runnerOn(core, second)..start();
      await pumpEventQueue();
      other.run(const ClaimOwner(7));
      await pumpEventQueue();
      expect(second.whereType<OwnerHeld>().single.effect, 7);
      expect(second.whereType<OwnerClaimed>(), isEmpty);
      jobs.dropOwnerStream();
      await pumpEventQueue();
      expect(
        second.whereType<OwnerChanged>().map((o) => o.held),
        [false],
      );
      await other.close();
    });

    test('a claim while the old owner\'s writes drain is owner_draining', () async {
      jobs.draining = true;
      runner.start();
      await pumpEventQueue();
      runner.run(const ClaimOwner(1));
      await pumpEventQueue();
      expect(only<OwnerDraining>().effect, 1);
    });

    test('a new stream after the old one ended loses the claim', () async {
      await claimed();
      core.end('stream_ended');
      await pumpEventQueue();
      expect(sent.whereType<OwnerChanged>().last.held, isFalse);
    });

    test('close releases the claim with its token', () async {
      final token = await claimed();
      await runner.close();
      final release = to('DELETE', '/api/jobs/owner').single;
      expect(ownerOf(release), token);
      expect(jobs.ownerToken, isNull);
    });
  });

  group('reads', () {
    test('LoadJobs reads the open jobs of the app, rows whole', () async {
      jobs.seed(
        'j1',
        state: 'running',
        step: 'turn:1',
        rev: 4,
        artifactId: 'abc',
        title: 'Timer',
        input: {
          'request': 'fix it',
          'base': 2,
          'threads': {'t1': 'c1'},
        },
        carry: {'stops': 1, 'can_keep_going': true, 'data': 'kept'},
        error: {'code': 'version_moved', 'detail': 3, 'step': 'publish:2'},
        steps: [
          {
            'key': 'turn:1',
            'result': {
              'base': 2,
              'notes': {
                'calls': [
                  {'id': 'w', 'tool': 'write_file', 'args': {'path': 'a'}},
                  {'id': 'a', 'tool': 'ask', 'question': 'which?'},
                ],
                'data': 'said',
              },
            },
            'version_n': null,
            'created_at': 'x',
          },
        ],
        questions: [
          {
            'question_id': 'q9',
            'tool_use_id': 'j1/turn:1/a',
            'answer': {'outcome': 'answered', 'answers': ['red']},
          },
        ],
      );
      jobs.seed('j2', state: 'done');
      runner.run(const LoadJobs(2));
      await pumpEventQueue();
      final r = to('GET', '/api/jobs').single;
      expect(Uri.parse(r.path).queryParameters, {'open': '1', 'client': 'app'});
      final j = only<JobsLoaded>().jobs.single;
      expect(j.id, 'j1');
      expect(j.state, JobState.running);
      expect(j.step, const TurnKey(1));
      expect(j.rev, 4);
      expect(j.input, const JobInput(request: 'fix it', base: 2, threads: {'t1': 'c1'}));
      expect(j.carry, const Carry(stops: 1, canKeepGoing: true, data: 'kept'));
      expect(j.error, const JobError('version_moved', 3, PublishKey(2)));
      expect(
        j.steps[const TurnKey(1)],
        const TurnStepResult(
          TurnNotes(
            calls: [
              OtherCall('w', 'write_file', args: {'path': 'a'}),
              AskCall('a', Question(data: 'which?')),
            ],
            data: 'said',
          ),
          base: 2,
        ),
      );
      expect(j.questionOf(1, 'a')?.answer, const Answered(['red']));
    });

    group('the JSON import', () {
      late Directory support;
      setUp(() {
        support = scratchRoot().createTempSync('runner-import-');
        addTearDown(() => support.deleteSync(recursive: true));
        File('${support.path}/pending-make.json')
            .writeAsStringSync('{"requests":["laps"],"waiting":[]}');
      });

      EffectRunner importing(List<String> tokens) => runnerOn(
        core,
        sent,
        importState: (c, token) {
          tokens.add(token);
          return importJsonState(
            supportDir: support.path,
            core: c,
            ownerToken: token,
          );
        },
      );

      test('LoadJobs first imports the files under the claim\'s token, then '
          'loads the jobs the import made', () async {
        core.on('POST', '/api/client-imports/{key}', (req, _) {
          jobs.seed('json-0123', state: 'queued');
          return FakeCore.json({
            'import': {
              'already': false,
              'jobs': 1,
              'consents': 0,
              'skipped': <String>[],
              'left_out': <String>[],
              'done_at': 'x',
            },
          });
        });
        await runner.close();
        final tokens = <String>[];
        runner = importing(tokens);
        final token = await claimed();
        runner.run(const LoadJobs(2));
        await pumpEventQueue();
        expect(tokens, [token]);
        final paths = [
          for (final r in core.requests) Uri.parse(r.path).path,
        ];
        expect(
          paths.indexOf('/api/client-imports/app-json-v1'),
          lessThan(paths.indexOf('/api/jobs')),
        );
        expect(only<JobsLoaded>().jobs.map((j) => j.id), ['json-0123']);
        expect(File('${support.path}/pending-make.json').existsSync(), isFalse);
      });

      test('an import the core refuses leaves the files, and the jobs load',
          () async {
        core.on(
          'POST',
          '/api/client-imports/{key}',
          (req, _) => FakeCore.error(500, 'internal', 'no'),
        );
        jobs.seed('j1', state: 'queued');
        await runner.close();
        runner = importing([]);
        await claimed();
        runner.run(const LoadJobs(2));
        await pumpEventQueue();
        expect(only<JobsLoaded>().jobs.map((j) => j.id), ['j1']);
        expect(File('${support.path}/pending-make.json').existsSync(), isTrue);
      });
    });

    test('ReadJob answers the row, or null for none', () async {
      jobs.seed('j', state: 'paused', rev: 3);
      runner
        ..run(const ReadJob(1, 'j'))
        ..run(const ReadJob(2, 'gone'));
      await pumpEventQueue();
      final reads = sent.whereType<JobRead>().toList();
      expect(reads.firstWhere((r) => r.effect == 1).job?.state, JobState.paused);
      expect(reads.firstWhere((r) => r.effect == 2).job, isNull);
    });
  });

  group('commits', () {
    test('CreateJob posts the job with the owner\'s token and answers its row', () async {
      final token = await claimed();
      runner.run(
        const CreateJob(
          1,
          'j',
          kind: JobKind.make,
          input: JobInput(request: 'a timer'),
          title: 'Timer',
        ),
      );
      await pumpEventQueue();
      final r = to('POST', '/api/jobs').single;
      expect(ownerOf(r), token);
      expect(body(r), {
        'id': 'j',
        'client': 'app',
        'kind': 'make',
        'title': 'Timer',
        'input': {'request': 'a timer', 'base': 0, 'threads': <String, Object?>{}},
      });
      final c = only<Committed>();
      expect(c.effect, 1);
      expect(c.job.state, JobState.queued);
      expect(c.job.rev, 1);
      expect(
        only<JobNoticed>(),
        const JobNoticed('j', state: JobState.queued, rev: 1),
      );
    });

    test('a write without the claim is not_owner', () async {
      runner.run(
        const CreateJob(1, 'j', kind: JobKind.make, input: JobInput(request: 'x')),
      );
      await pumpEventQueue();
      expect(ownerOf(to('POST', '/api/jobs').single), isNull);
      final r = only<Refused>();
      expect(r.reason, isA<NotOwner>());
      expect(jobs.jobs, isEmpty);
    });

    test('MoveJob patches at its rev, setting and clearing fields', () async {
      await claimed();
      jobs.seed('j', state: 'failed', rev: 5, error: {'code': 'turn_failed'});
      runner.run(
        const MoveJob(
          1,
          'j',
          ifRev: 5,
          to: JobState.running,
          error: Clear(),
          carry: SetTo(Carry(stops: 2, canKeepGoing: false)),
        ),
      );
      await pumpEventQueue();
      expect(body(to('PATCH', '/api/jobs/j').single), {
        'if_rev': 5,
        'state': 'running',
        'error': null,
        'carry': {'stops': 2, 'can_keep_going': false, 'data': null},
      });
      final c = only<Committed>();
      expect(c.job.state, JobState.running);
      expect(c.job.error, isNull);
      expect(c.job.carry, const Carry(stops: 2, canKeepGoing: false));
      expect(c.job.rev, 6);
    });

    test('a move at an old rev is job_moved, with the current row', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 5);
      runner.run(const MoveJob(1, 'j', ifRev: 4, to: JobState.paused));
      await pumpEventQueue();
      final r = only<Refused>();
      expect(r.reason, isA<JobMoved>());
      expect(r.job?.rev, 5);
      expect(r.job?.state, JobState.running);
    });

    test('a move the table refuses is Invalid(invalid_move)', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 5);
      runner.run(const MoveJob(1, 'j', ifRev: 5, to: JobState.queued));
      await pumpEventQueue();
      final reason = only<Refused>().reason;
      expect(reason, isA<Invalid>().having((i) => i.code, 'code', 'invalid_move'));
    });

    test('CommitStep commits turn:<n> with its notes and base, once', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 2);
      const notes = TurnNotes(
        calls: [
          FinishCall('f', PublishPlan(base: 3, data: {'request': {}}), reply: ReplyPlan(data: {'body': 'hi'})),
          RenderCall(
            'r',
            RenderSpec(
              draftId: 'd',
              revision: 2,
              kind: RenderKind.tests,
              digest: 'sha256:ab',
              files: ['tests/a.test.js'],
              migration: MigrationSpec(from: 1, to: 2),
            ),
          ),
        ],
        data: {'text': 'ok'},
      );
      runner.run(const CommitStep(1, 'j', 1, notes, base: 3));
      await pumpEventQueue();
      final r = to('POST', '/api/jobs/j/steps/turn:1').single;
      expect(body(r).keys, ['result']);
      final c = only<Committed>();
      expect(c.step, const TurnStepResult(notes, base: 3));
      expect(c.replayed, isFalse);
      expect(c.job.steps[const TurnKey(1)], const TurnStepResult(notes, base: 3));
      expect(c.job.step, const TurnKey(1));
      runner.run(const CommitStep(2, 'j', 1, notes, base: 3));
      await pumpEventQueue();
      final again = sent.whereType<Committed>().last;
      expect(again.replayed, isTrue);
      expect(again.job.rev, c.job.rev);
    });

    test('a make\'s Publish commits publish:<base> with state done', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 2);
      runner.run(
        const Publish(
          1,
          'j',
          2,
          'f',
          PublishPlan(base: 0, data: {'artifact_id': null, 'request': {'title': 'Timer'}}),
          done: true,
        ),
      );
      await pumpEventQueue();
      expect(body(to('POST', '/api/jobs/j/steps/publish:0').single), {
        'publish': {'artifact_id': null, 'request': {'title': 'Timer'}, 'base': 0},
        'state': 'done',
      });
      final c = only<Committed>();
      final step = c.step! as PublishStepResult;
      expect(step.version, 1);
      expect(c.job.artifactId, step.artifactId);
      expect(c.job.state, JobState.done);
      expect(sent.whereType<JobNoticed>().last.step, const PublishKey(0));
    });

    group('the debug kill hook', () {
      late Directory dir;
      setUp(() => dir = Directory.systemTemp.createTempSync('clax-kill-'));
      tearDown(() => dir.deleteSync(recursive: true));

      Future<void> publishHeldAt(KillPoint at) async {
        runner = runnerOn(
          core,
          sent,
          kill: KillHook(at, '${dir.path}/${KillHook.fileName}'),
        );
        await claimed();
        jobs.seed('j', state: 'running', rev: 2);
        runner.run(
          const Publish(
            1,
            'j',
            2,
            'f',
            PublishPlan(base: 0, data: {'artifact_id': null, 'request': {'title': 'Timer'}}),
            done: true,
          ),
        );
        await pumpEventQueue();
      }

      String killPoint() =>
          File('${dir.path}/${KillHook.fileName}').readAsStringSync();

      test('before the commit, it writes the kill point and sends nothing',
          () async {
        await publishHeldAt(KillPoint.beforePublishCommit);
        expect(killPoint(), 'before-publish-commit j\n');
        expect(to('POST', '/api/jobs/j/steps/publish:0'), isEmpty);
        expect(jobs.published, isEmpty);
        expect(sent.whereType<Committed>(), isEmpty);
      });

      test('after the commit, the core holds the version and the queue hears '
          'nothing', () async {
        await publishHeldAt(KillPoint.afterPublishCommit);
        expect(killPoint(), 'after-publish-commit j\n');
        expect(to('POST', '/api/jobs/j/steps/publish:0'), hasLength(1));
        expect(jobs.published, hasLength(1));
        expect(sent.whereType<Committed>(), isEmpty);
      });

      test('its points are named as CLAX_TEST_KILL_AT names them', () {
        expect(KillPoint.parse('before-publish-commit'), KillPoint.beforePublishCommit);
        expect(KillPoint.parse('after-publish-commit'), KillPoint.afterPublishCommit);
        expect(KillPoint.parse('elsewhere'), isNull);
        expect(KillPoint.parse(null), isNull);
      });
    });

    test('a Publish commits the plan the agent makes of its draft when it '
        'runs, at the effect\'s base', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 2);
      agent.planned = (p) async => PublishPlan(
        base: 9,
        data: {
          'artifact_id': null,
          'request': {'title': 'Now'},
        },
      );
      runner.run(const Publish(1, 'j', 2, 'f', PublishPlan(base: 0), done: true));
      await pumpEventQueue();
      expect(agent.resolved, [('j', 2, 'f')]);
      expect(body(to('POST', '/api/jobs/j/steps/publish:0').single), {
        'publish': {'artifact_id': null, 'request': {'title': 'Now'}, 'base': 0},
        'state': 'done',
      });
    });

    test('a publish whose draft the agent cannot make is refused no_draft, '
        'and sends nothing to the core', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 2);
      agent.planned = (_) async => throw const FormatException('gone');
      runner.run(const Publish(1, 'j', 2, 'f', PublishPlan(base: 0), done: true));
      await pumpEventQueue();
      expect(only<Refused>().reason, isA<Invalid>().having((i) => i.code, 'code', 'no_draft'));
      expect(to('POST', '/api/jobs/j/steps/publish:0'), isEmpty);
    });

    test('a publish on a moved base is VersionMoved(current), and records nothing', () async {
      await claimed();
      jobs
        ..versions['abc'] = 4
        ..seed('j', kind: 'change', state: 'running', artifactId: 'abc', rev: 2);
      runner.run(
        const Publish(1, 'j', 1, 'f', PublishPlan(base: 3, data: {'artifact_id': 'abc', 'request': {}}), done: false),
      );
      await pumpEventQueue();
      expect(only<Refused>().reason, const VersionMoved(4));
      expect(jobs.published, isEmpty);
    });

    test('a draft with no clean render is NotRenderedClean', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 2);
      runner.run(
        const Publish(
          1,
          'j',
          1,
          'f',
          PublishPlan(
            base: 0,
            data: {
              'request': {'title': 'T'},
              'draft': {'id': 'd', 'digest': 'abc'},
            },
          ),
          done: true,
        ),
      );
      await pumpEventQueue();
      expect(only<Refused>().reason, isA<NotRenderedClean>());
    });

    test('Reply commits the reply step and ends the change', () async {
      await claimed();
      jobs.sessions.addAll(['s0', 's1']);
      jobs.seed(
        'j',
        kind: 'change',
        state: 'running',
        rev: 3,
        publishedFrom: 's0',
        steps: [
          {'key': 'publish:1', 'result': {'artifact_id': 'abc', 'version': 2}, 'version_n': 2, 'created_at': 'x'},
        ],
      );
      runner.run(
        const Reply(1, 'j', 2, 'f', ReplyPlan(data: {'threads': ['t1'], 'body': 'Done.', 'resolve': true})),
      );
      await pumpEventQueue();
      expect(body(to('POST', '/api/jobs/j/steps/reply').single), {
        'reply': {'threads': ['t1'], 'body': 'Done.', 'resolve': true, 'session_id': 's1'},
        'state': 'done',
      });
      final c = only<Committed>();
      expect(c.job.state, JobState.done);
      expect(c.step, isA<ReplyStepResult>());
      expect(c.job.steps[const PublishKey(1)], const PublishStepResult(artifactId: 'abc', version: 2));
      expect(jobs.repliedFrom, ['s0'], reason: 'the publishing session is live');
    });

    /// Job `j`, a running change whose publish step made version 2 of
    /// `abc` from session [from] (none: a change imported from the app's
    /// ledger).
    void seedPublished({String? from}) => jobs.seed(
      'j',
      kind: 'change',
      state: 'running',
      rev: 3,
      artifactId: 'abc',
      publishedFrom: from,
      steps: [
        {'key': 'publish:1', 'result': {'artifact_id': 'abc', 'version': 2}, 'version_n': 2, 'created_at': 'x'},
      ],
    );

    const reply = Reply(1, 'j', 2, 'f', ReplyPlan(data: {'threads': ['t1'], 'body': 'Done.', 'resolve': false}));

    test('a Reply after the publishing session ended comes from the worker\'s session', () async {
      await claimed();
      jobs.sessions.addAll(['s0', 's1']);
      seedPublished(from: 's0');
      jobs.endSession('s0');
      runner.run(reply);
      await pumpEventQueue();
      expect(only<Committed>().job.state, JobState.done);
      expect(jobs.repliedFrom, ['s1']);
    });

    test('a Reply on a change whose version names no session comes from the worker\'s session', () async {
      await claimed();
      jobs.sessions.add('s1');
      seedPublished();
      runner.run(reply);
      await pumpEventQueue();
      expect(only<Committed>().job.state, JobState.done);
      expect(jobs.repliedFrom, ['s1']);
    });

    test('a Reply without the worker\'s session goes without one, and the core '
        'refuses it unknown_session when the publishing session ended', () async {
      runner.close();
      runner = EffectRunner(
        core: CoreTransport(core, timer: timers.make, retryDelay: (_) async {}),
        agent: agent,
        render: renders,
        ui: ui,
        probeClaude: () async => probe,
        session: () async => throw const SocketException('the core is away'),
        send: sent.add,
      );
      await claimed();
      jobs.sessions.add('s0');
      seedPublished(from: 's0');
      jobs.endSession('s0');
      runner.run(reply);
      await pumpEventQueue();
      expect(
        (body(to('POST', '/api/jobs/j/steps/reply').single)['reply']! as Map).containsKey('session_id'),
        isFalse,
      );
      expect((only<Refused>().reason as Invalid).code, 'unknown_session');
      expect(jobs.replied, isEmpty);
    });

    test('a Reply naming an ended session is refused unknown_session', () async {
      await claimed();
      jobs.sessions.addAll(['s0', 's1']);
      seedPublished(from: 's0');
      jobs.endSession('s1');
      runner.run(reply);
      await pumpEventQueue();
      expect((only<Refused>().reason as Invalid).code, 'unknown_session');
    });

    test('Ask creates the question with its tool call, then the job waits on it', () async {
      final token = await claimed();
      jobs.seed('j', state: 'running', rev: 2);
      const ask = Ask(1, 'j', 2, 'a1', Question(data: {'questions': [{'question': 'Which?'}]}));
      runner.run(ask);
      await pumpEventQueue();
      final created = to('POST', '/api/sessions/s1/questions').single;
      expect(body(created), {
        'questions': [{'question': 'Which?'}],
        'source': 'ask',
        'tool_use_id': 'j/turn:2/a1',
      });
      final put = core.requests.where((r) => r.method == 'PUT').single;
      expect(put.path, startsWith('/api/jobs/j/questions/'));
      expect(ownerOf(put), token);
      final c = only<Committed>();
      expect(c.job.state, JobState.waiting);
      expect(c.job.questionOf(2, 'a1')?.answer, const AnswerPending());

      // Asked again (a worker resumed after a kill), the same question.
      runner.run(const Ask(2, 'j', 2, 'a1', Question(data: {'questions': [{'question': 'Which?'}]})));
      await pumpEventQueue();
      expect(jobs.questions, hasLength(1));
      expect(sent.whereType<Committed>().last.job.questions, hasLength(1));
    });

    test('a commit with no answer in its time reads the job back', () async {
      await claimed();
      jobs.seed('j', state: 'running', rev: 2);
      jobs.stepGate = Completer<void>();
      runner.run(const CommitStep(1, 'j', 1, TurnNotes()));
      await pumpEventQueue();
      timers.fire(const Duration(seconds: 125));
      await pumpEventQueue();
      final read = only<JobRead>();
      expect(read.effect, 1);
      expect(read.job?.rev, 2);
      expect(sent.whereType<Committed>(), isEmpty);
    });
  });

  group('work', () {
    test('StartTurn runs one turn and answers TurnDone', () async {
      const out = TurnOutcome(TurnNotes(calls: [OtherCall('w', 'write_file')]));
      agent.turns.add((_) async => out);
      const input = TurnInput(turn: 1, input: JobInput(request: 'x'), base: 0);
      runner.run(const StartTurn(1, 'j', input));
      await pumpEventQueue();
      expect(agent.inputs.single, input);
      expect(only<TurnDone>().outcome, out);
    });

    test('a turn that cannot reach Claude is TurnFailed(network)', () async {
      agent.turns.add(
        (_) async => throw const AgentError('offline', kind: AgentErrorKind.network),
      );
      runner.run(
        const StartTurn(1, 'j', TurnInput(turn: 1, input: JobInput(request: 'x'), base: 0)),
      );
      await pumpEventQueue();
      expect(only<TurnFailed>().failure, TurnFailure.network);
    });

    test('a turn that fails otherwise is TurnFailed(other) with why', () async {
      agent.turns.add((_) async => throw const AgentError('refused'));
      runner.run(
        const StartTurn(1, 'j', TurnInput(turn: 1, input: JobInput(request: 'x'), base: 0)),
      );
      await pumpEventQueue();
      final f = only<TurnFailed>();
      expect(f.failure, TurnFailure.other);
      expect(f.detail, 'refused');
    });

    test('cancelJob stops a turn in flight and sends nothing for it', () async {
      final answer = Completer<TurnOutcome>();
      var stopped = false;
      agent.turns.add((cancelled) {
        cancelled.then((_) => stopped = true);
        return answer.future;
      });
      runner.run(
        const StartTurn(1, 'j', TurnInput(turn: 1, input: JobInput(request: 'x'), base: 0)),
      );
      await pumpEventQueue();
      runner.run(const StopWork(2, 'j'));
      await pumpEventQueue();
      expect(stopped, isTrue);
      answer.complete(const TurnOutcome(TurnNotes()));
      await pumpEventQueue();
      expect(sent, isEmpty);
    });

    test('RunTool answers ToolDone; an exception is an error result', () async {
      agent.tools.add((_) async => const ToolResult(data: 'ok'));
      agent.tools.add((_) async => throw const FormatException('bad path'));
      runner
        ..run(const RunTool(1, 'j', 1, OtherCall('a', 'read_file')))
        ..run(const RunTool(2, 'j', 1, OtherCall('b', 'read_file')));
      await pumpEventQueue();
      final done = sent.whereType<ToolDone>().toList();
      expect(done.firstWhere((d) => d.callId == 'a').result, const ToolResult(data: 'ok'));
      expect(done.firstWhere((d) => d.callId == 'b').result.isError, isTrue);
    });

    test('a Render is recorded in the core under its spec\'s digest before '
        'Rendered is sent', () async {
      const spec = RenderSpec(draftId: 'd1', revision: 2, digest: 'sha256:ab');
      const outcome = RenderOutcome(
        clean: false,
        data: {
          'errors': ['x is undefined'],
          'failed_tests': ['adds'],
        },
      );
      final answer = Completer<RenderResult>();
      renders.answers.add(answer.future);
      runner.run(const Render(1, 'j', 1, 'r', spec));
      await pumpEventQueue();
      expect(renders.asked.single, const RenderRequest(1, 'j', 'r', spec));
      expect(sent, isEmpty);
      answer.complete(const RenderAnswered(1, 'j', outcome));
      await pumpEventQueue();
      expect(jobs.renders.single, {
        'draft': 'd1',
        'digest': 'sha256:ab',
        'errors': ['x is undefined'],
        'failed_tests': ['adds'],
      });
      expect(only<Rendered>().outcome, outcome);
    });

    test('a render draws the spec the agent serves when it runs, and is '
        'recorded under that spec\'s digest', () async {
      const planned = RenderSpec(draftId: '', revision: 0);
      const spec = RenderSpec(draftId: 'd2', revision: 4, digest: 'sha256:cd', files: ['tests/a.test.js']);
      agent.served = (p) async {
        expect(p, planned);
        return spec;
      };
      renders.answers.add(Future.value(const RenderAnswered(1, 'j', RenderOutcome(clean: true))));
      runner.run(const Render(1, 'j', 3, 'r', planned));
      await pumpEventQueue();
      expect(agent.resolved, [('j', 3, 'r')]);
      expect(renders.asked.single, const RenderRequest(1, 'j', 'r', spec));
      expect(jobs.renders.single['draft'], 'd2');
      expect(jobs.renders.single['digest'], 'sha256:cd');
      expect(only<Rendered>().outcome.clean, isTrue);
    });

    test('a draft that cannot be served answers the render as failed, and '
        'asks the UI nothing', () async {
      agent.served = (_) async => throw const FormatException('no draft');
      runner.run(const Render(1, 'j', 1, 'r', RenderSpec(draftId: '', revision: 0)));
      await pumpEventQueue();
      final r = only<Rendered>();
      expect(r.outcome.clean, isFalse);
      expect(r.outcome.errors.single, contains('no draft'));
      expect(renders.asked, isEmpty);
      expect(jobs.renders, isEmpty);
    });

    test('a render past the run\'s renders answers the render as failed '
        'with the refusal, and the worker goes on', () async {
      // The queue's own agent, on a run that may render no more.
      final turns = AgentTurns(
        backend: _NoTurns(),
        open: (_) async => AgentJob(
          input: const RunInput(system: '', first: []),
          toolbox: toolboxWith(
            files: const {},
            kind: RequestKind.newApp,
            caps: const RunCaps(
              turns: 8,
              renders: 0,
              outputTokens: 32000,
              wallClock: Duration(seconds: 90),
            ),
            extra: queueTools(),
          ),
        ),
        serveDraft: (_, _) async => 'd1',
      );
      await runner.close();
      runner = runnerOn(core, sent, agentPort: turns);
      runner.run(const Render(1, 'j', 1, 'r', RenderSpec(draftId: '', revision: 0)));
      await pumpEventQueue();
      final r = only<Rendered>();
      expect(r.outcome.clean, isFalse);
      expect(r.outcome.errors.single, contains('used its 0 renders'));
      expect(renders.asked, isEmpty);
      runner.run(const ProbeClaude(2));
      await pumpEventQueue();
      expect(only<ClaudeReachable>().reachable, isTrue);
    });

    test('a render not clean that names nothing is recorded with an error, '
        'so it never clears a publish', () async {
      renders.answers.add(
        Future.value(
          const RenderAnswered(
            1,
            'j',
            RenderOutcome(clean: false, data: {'error': 'did not load'}),
          ),
        ),
      );
      runner.run(
        const Render(1, 'j', 1, 'r', RenderSpec(draftId: 'd1', revision: 2, digest: 'sha256:ab')),
      );
      await pumpEventQueue();
      expect(jobs.renders.single['errors'], ['did not load']);
    });

    test('a render without a digest is not recorded', () async {
      renders.answers.add(
        Future.value(const RenderAnswered(1, 'j', RenderOutcome(clean: true))),
      );
      runner.run(const Render(1, 'j', 1, 'r', RenderSpec(draftId: 'd', revision: 1)));
      await pumpEventQueue();
      expect(only<Rendered>().effect, 1);
      expect(jobs.renders, isEmpty);
    });

    test('a render the UI lost is RenderLost, and records nothing', () async {
      renders.answers.add(Future.value(const RenderGone(1, 'j')));
      runner.run(
        const Render(1, 'j', 1, 'r', RenderSpec(draftId: 'd', revision: 1, digest: 'sha256:ab')),
      );
      await pumpEventQueue();
      expect(only<RenderLost>().effect, 1);
      expect(jobs.renders, isEmpty);
    });

    test('StopWork drops a render in flight and cancels the job\'s renders', () async {
      final answer = Completer<RenderResult>();
      renders.answers.add(answer.future);
      runner.run(
        const Render(1, 'j', 1, 'r', RenderSpec(draftId: 'd', revision: 1, digest: 'sha256:ab')),
      );
      await pumpEventQueue();
      runner.run(const StopWork(2, 'j'));
      expect(renders.cancelled, ['j']);
      answer.complete(const RenderAnswered(1, 'j', RenderOutcome(clean: true)));
      await pumpEventQueue();
      expect(sent, isEmpty);
      expect(jobs.renders, isEmpty);
    });

    test('the agent forgets a job at StopWork, at a final row, and when the '
        'core holds none', () async {
      runner.run(const StopWork(1, 'a'));
      expect(agent.forgot, ['a']);
      await claimed();
      jobs.seed('b', state: 'running', rev: 2);
      runner.run(const MoveJob(2, 'b', ifRev: 2, to: JobState.cancelled));
      await pumpEventQueue();
      expect(only<Committed>().job.state, JobState.cancelled);
      runner.run(const ReadJob(3, 'gone'));
      await pumpEventQueue();
      expect(agent.forgot, ['a', 'b', 'gone']);
    });

    test('ProbeClaude answers ClaudeReachable', () async {
      probe = false;
      runner.run(const ProbeClaude(1));
      await pumpEventQueue();
      expect(only<ClaudeReachable>().reachable, isFalse);
    });
  });

  test('Notice, KeepAlive and PublishState go to the UI port', () {
    const state = QueueState([]);
    runner
      ..run(const Notice(1, NoticeKind.made, 'j', 'Timer'))
      ..run(const KeepAlive(2, true))
      ..run(const PublishState(3, state));
    expect(ui.said, const [
      Notice(1, NoticeKind.made, 'j', 'Timer'),
      KeepAlive(2, true),
      PublishState(3, state),
    ]);
    expect(sent, isEmpty);
  });

  test("a worker's runner subscribes, claims and speaks to the UI through its context", () async {
    final port = ReceivePort();
    final out = <ToUi>[];
    final context = WorkerContext(
      WorkerStart(
        toUi: port.sendPort,
        coreHandle: 1,
        supportDir: '',
        settings: const {},
        apiKey: null,
        runner: _noRunner,
        attach: (_) => core,
      ),
      sent.add,
      out.add,
    );
    final WorkerRunner worker = EffectRunner.forWorker(
      context,
      agent: agent,
      probeClaude: () async => true,
      session: () async => 's1',
    );
    await pumpEventQueue();
    worker.run(const ClaimOwner(0));
    worker.run(const KeepAlive(1, true));
    const spec = RenderSpec(draftId: 'd', revision: 1);
    worker.run(const Render(2, 'j', 1, 'r', spec));
    await pumpEventQueue();
    expect(body(to('POST', '/api/jobs/owner').single)['stream'], 'fake-1');
    expect(only<OwnerClaimed>().token, jobs.ownerToken);
    // Its renders go out through the context's port, as the codec's
    // envelopes, and the port's answer is the runner's.
    expect(out, const [
      UiEffect(KeepAlive(1, true)),
      UiRender(RenderRequest(2, 'j', 'r', spec)),
    ]);
    context.render.answer(const RenderGone(2, 'j'));
    await pumpEventQueue();
    expect(only<RenderLost>().effect, 2);
    await (worker as EffectRunner).close();
    port.close();
  });

  test('nothing is sent after close', () async {
    final answer = Completer<ToolResult>();
    agent.tools.add((_) => answer.future);
    runner.run(const RunTool(1, 'j', 1, OtherCall('a', 'read_file')));
    await pumpEventQueue();
    await runner.close();
    answer.complete(const ToolResult());
    runner.run(const ProbeClaude(2));
    await pumpEventQueue();
    expect(sent, isEmpty);
  });

  test('the core\'s job events are JobNoticed, its owner events OwnerChanged', () async {
    await claimed();
    jobs.seed('j', state: 'waiting', rev: 3);
    final qid = (jobs.questions['q0'] = {
      'id': 'q0',
      'session': 's1',
      'tool_use_id': 'j/turn:1/a',
      'outcome': null,
      'answers': null,
    })['id']! as String;
    jobs.jobs['j']!['questions'] = [
      {'question_id': qid, 'tool_use_id': 'j/turn:1/a', 'answer': {'outcome': 'pending'}},
    ];
    jobs.closeQuestion(qid, 'answered', ['red']);
    await pumpEventQueue();
    expect(
      sent.whereType<JobNoticed>().single,
      const JobNoticed('j', state: JobState.running, rev: 4),
    );
    expect(jsonEncode(sent.whereType<OwnerChanged>().map((o) => o.held).toList()), '[true]');
  });
}

class _Agent implements AgentPort {
  final List<Future<TurnOutcome> Function(Future<void> cancelled)> turns = [];
  final List<Future<ToolResult> Function(Future<void> cancelled)> tools = [];
  final List<TurnInput> inputs = [];

  @override
  Future<TurnOutcome> turn(JobId jobId, TurnInput input, Future<void> cancelled) {
    inputs.add(input);
    return turns.removeAt(0)(cancelled);
  }

  @override
  Future<ToolResult> tool(JobId jobId, int turn, OtherCall call, Future<void> cancelled) =>
      tools.removeAt(0)(cancelled);

  /// What [renderSpec] and [publishPlan] answer; as planned when unset.
  Future<RenderSpec> Function(RenderSpec planned)? served;
  Future<PublishPlan> Function(PublishPlan planned)? planned;
  final List<(JobId, int, String)> resolved = [];

  @override
  Future<RenderSpec> renderSpec(JobId jobId, int turn, String callId, RenderSpec planned) {
    resolved.add((jobId, turn, callId));
    return served?.call(planned) ?? Future.value(planned);
  }

  @override
  Future<PublishPlan> publishPlan(JobId jobId, int turn, String callId, PublishPlan planned) {
    resolved.add((jobId, turn, callId));
    return this.planned?.call(planned) ?? Future.value(planned);
  }

  /// What each render drew, as the runner told it.
  final List<(JobId, int, String, RenderOutcome)> drawn = [];

  @override
  void rendered(JobId jobId, int turn, String callId, RenderOutcome outcome) =>
      drawn.add((jobId, turn, callId, outcome));

  /// The jobs the runner told it to forget, in order.
  final List<JobId> forgot = [];

  @override
  void forget(JobId jobId) => forgot.add(jobId);
}

/// A render port answering from [answers], in order; a render with none
/// queued fails the test.
/// A model never asked.
class _NoTurns implements TurnBackend {
  @override
  String get name => 'none';

  @override
  Future<ModelTurn> turn(TurnRequest r) =>
      throw UnimplementedError('no turn is asked');
}

class _Render implements RenderPort {
  final List<Future<RenderResult>> answers = [];
  final List<RenderRequest> asked = [];
  final List<JobId> cancelled = [];

  @override
  Future<RenderResult> render(RenderRequest request) {
    asked.add(request);
    if (answers.isEmpty) throw StateError('no answer queued for $request');
    return answers.removeAt(0);
  }

  @override
  void cancel(JobId jobId) => cancelled.add(jobId);
}

class _Ui implements UiPort {
  final List<Effect> said = [];

  @override
  void notice(Notice notice) => said.add(notice);

  @override
  void keepAlive(KeepAlive keepAlive) => said.add(keepAlive);

  @override
  void publishState(PublishState state) => said.add(state);
}

/// The transport's deadline timers, which fire only when a test fires
/// them.
class _Timers {
  final List<(Duration, void Function(), _HeldTimer)> held = [];

  Timer make(Duration d, void Function() f) {
    final t = _HeldTimer();
    held.add((d, f, t));
    return t;
  }

  /// Fires every live timer of duration [d].
  void fire(Duration d) {
    for (final (dd, f, t) in [...held]) {
      if (dd == d && t.isActive) {
        t.cancel();
        f();
      }
    }
  }
}

class _HeldTimer implements Timer {
  bool _active = true;
  @override
  void cancel() => _active = false;
  @override
  bool get isActive => _active;
  @override
  int get tick => 0;
}

WorkerRunner _noRunner(WorkerContext _) => throw UnimplementedError();
