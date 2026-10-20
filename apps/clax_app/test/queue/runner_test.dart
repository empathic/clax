import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/runner.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_core.dart';
import '../support/fake_core_jobs.dart';

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

  EffectRunner runnerOn(FakeCore c, List<Msg> out) => EffectRunner(
    core: CoreTransport(
      c,
      timer: timers.make,
      retryDelay: (_) async {},
    ),
    agent: agent,
    render: renders,
    ui: ui,
    probeClaude: () async => probe,
    session: () async => 's1',
    send: out.add,
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
          RenderCall('r', RenderSpec(draftId: 'd', revision: 2, kind: RenderKind.tests)),
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
      jobs.seed(
        'j',
        kind: 'change',
        state: 'running',
        rev: 3,
        steps: [
          {'key': 'publish:1', 'result': {'artifact_id': 'abc', 'version': 2}, 'version_n': 2, 'created_at': 'x'},
        ],
      );
      runner.run(
        const Reply(1, 'j', 2, 'f', ReplyPlan(data: {'threads': ['t1'], 'body': 'Done.', 'resolve': true})),
      );
      await pumpEventQueue();
      expect(body(to('POST', '/api/jobs/j/steps/reply').single), {
        'reply': {'threads': ['t1'], 'body': 'Done.', 'resolve': true},
        'state': 'done',
      });
      final c = only<Committed>();
      expect(c.job.state, JobState.done);
      expect(c.step, isA<ReplyStepResult>());
      expect(c.job.steps[const PublishKey(1)], const PublishStepResult(artifactId: 'abc', version: 2));
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

    test('a Render is recorded in the core before Rendered is sent', () async {
      const spec = RenderSpec(draftId: 'd1', revision: 2);
      const outcome = RenderOutcome(
        clean: true,
        data: {'digest': 'abc', 'errors': [], 'failed_tests': []},
      );
      final answer = Completer<RenderOutcome?>();
      renders.answers.add(answer.future);
      runner.run(const Render(1, 'j', 1, 'r', spec));
      await pumpEventQueue();
      expect(renders.asked.single, ('j', spec));
      answer.complete(outcome);
      await pumpEventQueue();
      expect(jobs.renders.single, {
        'draft': 'd1',
        'digest': 'abc',
        'errors': <Object?>[],
        'failed_tests': <Object?>[],
      });
      expect(only<Rendered>().outcome, outcome);
    });

    test('a render the UI lost is RenderLost, and records nothing', () async {
      renders.answers.add(Future.value(null));
      runner.run(const Render(1, 'j', 1, 'r', RenderSpec(draftId: 'd', revision: 1)));
      await pumpEventQueue();
      expect(only<RenderLost>().effect, 1);
      expect(jobs.renders, isEmpty);
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
    expect(ui.said, [
      ('notice', NoticeKind.made, 'j', 'Timer'),
      ('keepAlive', true),
      ('state', state),
    ]);
    expect(sent, isEmpty);
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
}

class _Render implements RenderPort {
  final List<Future<RenderOutcome?>> answers = [];
  final List<(JobId, RenderSpec)> asked = [];

  @override
  Future<RenderOutcome?> render(JobId jobId, RenderSpec spec) {
    asked.add((jobId, spec));
    return answers.removeAt(0);
  }
}

class _Ui implements UiPort {
  final List<Object> said = [];

  @override
  void notice(NoticeKind kind, JobId jobId, String title) =>
      said.add(('notice', kind, jobId, title));

  @override
  void keepAlive(bool on) => said.add(('keepAlive', on));

  @override
  void publishState(QueueState state) => said.add(('state', state));
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
