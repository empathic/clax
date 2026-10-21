import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart' show DbOp;
import 'package:clax_app/agent/prompt.dart' show KeepGoingNote;
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/messages_runner.dart' show finishNudge;
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/finish.dart' show iconAsk;
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/queue/agent_port.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/runner.dart' show PublishRefused;
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../agent/tools/support.dart';

/// The queue's agent over lib/agent's parts: one model turn per `turn`, the
/// job's toolbox per `tool`, the draft served or published only when the
/// queue runs the call, `finish`'s rules at the publish, the run's limits
/// before each turn, and a job rebuilt from its rows alone.
void main() {
  late _Turns backend;
  late List<(JobId, DraftSnapshot)> served;
  late int opened;
  late List<KeepGoingNote> notes;
  late List<Toolbox> boxes;

  const page = '<title>Todo</title><p>todo</p>';
  const job = JobInput(request: 'a todo app');
  const change = JobInput(request: 'x', base: 3, threads: {'t1': 'c1'});

  /// A toolbox over [files] with the file tools and the queue's own.
  Toolbox box({
    Map<String, String> files = const {},
    RequestKind kind = RequestKind.newApp,
    RunCaps caps = testNewCaps,
    bool asksIcon = false,
    Draft? draft,
    RunBudget? budget,
  }) {
    final clock = StepClock();
    final change = kind == RequestKind.change;
    return Toolbox(
      RunState(
        ctx: RunContext(
          kind: kind,
          artifactId: change ? 'a1' : null,
          baseVersion: change ? 3 : 0,
          title: change ? 'Todo' : null,
          files: files,
          threadIds: change ? const ['t1'] : const [],
          clock: clock,
          request: 'a todo app',
          asksIcon: asksIcon,
        ),
        draft:
            draft ??
            Draft(
              files: files,
              title: change ? 'Todo' : null,
              baseVersion: change ? 3 : 0,
            ),
        budget: budget ?? RunBudget(caps, 'claude-haiku-5-5', clock),
      ),
      [...fileTools(), ...queueTools()],
    );
  }

  AgentTurns agentOn({
    Map<String, String> files = const {},
    RequestKind kind = RequestKind.newApp,
    RunCaps caps = testNewCaps,
    bool asksIcon = false,
    Job? row,
    double? capUsd,
  }) => AgentTurns(
    backend: backend,
    open: (jobId) async {
      opened++;
      final first = box(
        files: files,
        kind: kind,
        caps: caps,
        asksIcon: asksIcon,
      );
      boxes.add(first);
      return AgentJob(
        input: const RunInput(
          system: 'You make apps.',
          first: [
            {'type': 'text', 'text': 'a todo app'},
          ],
        ),
        toolbox: first,
        row: row,
        capUsd: capUsd,
        again: KeepGoingRun(
          first: (note, files) {
            notes.add(note);
            return [
              {'type': 'text', 'text': 'keep going over ${files.length}'},
            ];
          },
          toolbox: (draft, budget) =>
              box(files: files, kind: kind, draft: draft, budget: budget),
        ),
      );
    },
    serveDraft: (jobId, snap) async {
      served.add((jobId, snap));
      return 'd${served.length}';
    },
    session: () async => 's1',
  );

  setUp(() {
    backend = _Turns();
    served = [];
    opened = 0;
    notes = [];
    boxes = [];
  });

  /// Runs turn [n]'s calls of [agent] as the queue does, in order: each
  /// tool on the toolbox, each render served and drawn clean unless
  /// [dirty]. Answers the results by call ID.
  Future<Map<String, ToolResult>> runCalls(
    AgentTurns agent,
    int n,
    TurnNotes notes, {
    bool dirty = false,
  }) async {
    final out = <String, ToolResult>{};
    for (final c in notes.calls) {
      switch (c) {
        case OtherCall():
          out[c.id] = await agent.tool('j', n, c, Future<void>.value());
        case RenderCall(:final spec):
          await agent.renderSpec('j', n, c.id, spec);
          final o = RenderOutcome(
            clean: !dirty,
            data: {
              'inspection': {'title': 'Todo'},
              'errors': dirty ? ['boom'] : <String>[],
              'failed_tests': <String>[],
            },
          );
          agent.rendered('j', n, c.id, o);
          out[c.id] = ToolResult.rendered(o);
        case AskCall() || FinishCall():
          break;
      }
    }
    return out;
  }

  test('a turn\'s calls are the queue\'s; the draft is served and published '
      'as the calls before them left it, under finish\'s rules', () async {
    final agent = agentOn();
    backend.answer([
      {'type': 'text', 'text': 'Writing it.'},
      _use('w', 'write_file', {'path': 'index.html', 'content': page}),
      _use('t', 'write_file', {
        'path': 'tests/a.test.js',
        'content': 'test("a", () => {})',
      }),
      _use('m', 'set_meta', {'title': 'Todo'}),
      _use('r', 'render', {}),
      _use('f', 'finish', {'reply': 'Made it.'}),
    ]);
    final out = await agent.turn(
      'j',
      const TurnInput(turn: 1, input: job, base: 0),
      Future<void>.value(),
    );
    final calls = out.notes.calls;
    expect(calls.map((c) => c.id), ['w', 't', 'm', 'r', 'f']);
    expect(calls[3], isA<RenderCall>());
    expect(
      calls[4],
      const FinishCall(
        'f',
        PublishPlan(
          base: 0,
          data: {
            'finish': {'reply': 'Made it.'},
          },
        ),
      ),
      reason: 'a make replies on no threads; the call keeps its arguments',
    );
    expect(served, isEmpty, reason: 'nothing is served before the call runs');

    await runCalls(agent, 1, out.notes);
    final files = {
      'index.html': page,
      'tests/a.test.js': 'test("a", () => {})',
    };
    expect(served.single.$2.files, files);

    final plan = await agent.publishPlan(
      'j',
      1,
      'f',
      (calls[4] as FinishCall).publish,
    );
    expect(plan.base, 0);
    expect(plan.data, {
      'artifact_id': null,
      'request': {
        'title': 'Todo',
        'note': 'Made it.',
        'files': {
          'index.html': {'content': page},
          'tests/a.test.js': {'content': 'test("a", () => {})'},
        },
      },
      'draft': {'id': 'd1', 'digest': draftDigest(files)},
      'session_id': 's1',
    });
    expect(opened, 1);
  });

  test('a job whose open failed is opened again at its next turn', () async {
    var fails = 1;
    final agent = AgentTurns(
      backend: backend,
      open: (jobId) async {
        opened++;
        if (fails-- > 0) throw const FormatException('the core was busy');
        return AgentJob(
          input: const RunInput(system: 'You make apps.', first: []),
          toolbox: box(),
        );
      },
      serveDraft: (_, _) async => 'd1',
    );
    const input = TurnInput(turn: 1, input: job, base: 0);
    await expectLater(
      agent.turn('j', input, Future<void>.value()),
      throwsA(isA<FormatException>()),
    );
    backend.answer([
      _use('w', 'write_file', {'path': 'index.html', 'content': page}),
    ]);
    final out = await agent.turn('j', input, Future<void>.value());
    expect(out.notes.calls.single.id, 'w');
    expect(opened, 2);
  });

  test('the digest leaves out icon.svg', () {
    expect(
      draftDigest({'index.html': page, 'icon.svg': '<svg/>'}),
      draftDigest({'index.html': page}),
    );
    expect(
      draftDigest({'index.html': page}),
      isNot(draftDigest({'index.html': '$page '})),
    );
  });

  group('a publish refused before the core is asked', () {
    Future<Object?> refusal(
      AgentTurns agent,
      Map<String, Object?> args, {
      int turn = 1,
    }) async {
      try {
        await agent.publishPlan(
          'j',
          turn,
          'f',
          PublishPlan(base: 0, data: {'finish': args}),
        );
        return null;
      } on PublishRefused catch (e) {
        return e.code;
      }
    }

    Future<AgentTurns> written({bool asksIcon = false}) async {
      final agent = agentOn(asksIcon: asksIcon);
      backend.answer([
        _use('w', 'write_file', {'path': 'index.html', 'content': page}),
        _use('m', 'set_meta', {'title': 'Todo'}),
      ]);
      final one = await agent.turn(
        'j',
        const TurnInput(turn: 1, input: job, base: 0),
        Future<void>.value(),
      );
      await runCalls(agent, 1, one.notes);
      return agent;
    }

    test('a draft never rendered, or changed since, is not published', () async {
      final agent = await written();
      expect(await refusal(agent, {'reply': 'Done.'}), 'not_rendered');
      await agent.renderSpec('j', 1, 'r', const RenderSpec(draftId: '', revision: 0));
      agent.rendered('j', 1, 'r', const RenderOutcome(clean: true));
      expect(await refusal(agent, {'reply': 'Done.'}), isNull);
      await agent.tool(
        'j',
        1,
        const OtherCall('w2', 'write_file', args: {'path': 'app.js', 'content': '1'}),
        Future<void>.value(),
      );
      expect(await refusal(agent, {'reply': 'Done.'}), 'not_rendered');
    });

    test('a render with errors refuses it, and a reply is needed', () async {
      final agent = await written();
      await agent.renderSpec('j', 1, 'r', const RenderSpec(draftId: '', revision: 0));
      agent.rendered(
        'j',
        1,
        'r',
        const RenderOutcome(clean: false, data: {'errors': ['boom']}),
      );
      expect(await refusal(agent, {'reply': 'Done.'}), 'render_errors');
      expect(await refusal(agent, {'reply': ' '}), 'invalid_args');
    });

    test('a new app is asked for its icon once', () async {
      final agent = await written(asksIcon: true);
      await agent.renderSpec('j', 1, 'r', const RenderSpec(draftId: '', revision: 0));
      agent.rendered('j', 1, 'r', const RenderOutcome(clean: true));
      Object? asked;
      try {
        await agent.publishPlan(
          'j',
          1,
          'f',
          const PublishPlan(base: 0, data: {'finish': {'reply': 'Done.'}}),
        );
      } on PublishRefused catch (e) {
        asked = e.message;
      }
      expect(asked, iconAsk);
      expect(await refusal(agent, {'reply': 'Done.'}), isNull);
    });
  });

  test('a change\'s finish carries its reply; its publish sends what changed '
      'and its staged document writes, pinned', () async {
    final agent = agentOn(
      files: const {'index.html': page, 'app.js': 'old'},
      kind: RequestKind.change,
      caps: testChangeCaps,
    );
    backend.answer([
      _use('w', 'write_file', {'path': 'app.js', 'content': 'new'}),
      _use('r', 'render', {}),
      _use('f', 'finish', {'reply': 'Done.', 'resolve': true}),
    ]);
    final out = await agent.turn(
      'j',
      const TurnInput(turn: 1, input: change, base: 3),
      Future<void>.value(),
    );
    final finish = out.notes.calls.last as FinishCall;
    expect(
      finish.reply,
      const ReplyPlan(
        data: {
          'threads': ['t1'],
          'body': 'Done.',
          'resolve': true,
        },
      ),
    );
    await agent.tool(
      'j',
      1,
      out.notes.calls.first as OtherCall,
      Future<void>.value(),
    );
    // A staged write, pinned to the version the run read.
    final d = boxes.single.state.draft;
    d.pins['todos/a'] = 4;
    d.stage(const [
      DbOp(op: 'set', collection: 'todos', docId: 'a', data: {'t': 1}),
    ]);
    await agent.renderSpec('j', 1, 'r', const RenderSpec(draftId: '', revision: 0));
    agent.rendered('j', 1, 'r', const RenderOutcome(clean: true));
    final plan = await agent.publishPlan('j', 1, 'f', finish.publish);
    expect(plan.base, 3);
    final data = plan.data! as Map;
    expect(data['artifact_id'], 'a1');
    expect(data['request'], {
      'note': 'Done.',
      'files': {
        'index.html': {'content': page},
        'app.js': {'content': 'new'},
      },
      'addresses': ['t1'],
    });
    expect(data['docs'], [
      {
        'op': 'set',
        'path': 'todos/a',
        'data': {'t': 1},
        'if_version': 4,
      },
    ]);
    expect(data['session_id'], 's1');
  });

  test('the next turn sends the run\'s history: each turn and its calls\' '
      'results in order, a render\'s screenshot as an image', () async {
    final agent = agentOn();
    final first = [
      _use('w', 'write_file', {'path': 'index.html', 'content': page}),
      _use('r', 'render', {}),
    ];
    backend
      ..answer(first)
      ..answer([_use('f', 'finish', {})]);
    final one = await agent.turn(
      'j',
      const TurnInput(turn: 1, input: job, base: 0),
      Future<void>.value(),
    );
    final png = base64Encode([1, 2, 3]);
    await agent.turn(
      'j',
      TurnInput(
        turn: 2,
        input: job,
        base: 0,
        previous: one.notes,
        results: {
          'r': ToolResult(
            data: {
              'inspection': {'title': 'Todo', 'png': png},
            },
          ),
          'w': const ToolResult(data: {'ok': true}),
        },
      ),
      Future<void>.value(),
    );
    final sent = backend.requests.last.messages;
    expect(sent, hasLength(3));
    expect(sent[0]['role'], 'user');
    expect(sent[1], {'role': 'assistant', 'content': first});
    final results = sent[2]['content']! as List;
    expect([for (final r in results) (r as Map)['tool_use_id']], ['w', 'r']);
    final render = results[1] as Map;
    expect(render['content'], [
      {
        'type': 'text',
        'text': jsonEncode({
          'inspection': {'title': 'Todo'},
        }),
      },
      {
        'type': 'image',
        'source': {'type': 'base64', 'media_type': 'image/png', 'data': png},
      },
    ]);
  });

  test('a turn that called no tool is followed by the nudge', () async {
    final agent = agentOn();
    backend
      ..answer([
        {'type': 'text', 'text': 'Thinking.'},
      ])
      ..answer([_use('f', 'finish', {})]);
    final one = await agent.turn(
      'j',
      const TurnInput(turn: 1, input: job, base: 0),
      Future<void>.value(),
    );
    expect(one.notes.calls, isEmpty);
    await agent.turn(
      'j',
      TurnInput(turn: 2, input: job, base: 0, previous: one.notes),
      Future<void>.value(),
    );
    final last = backend.requests.last.messages.last;
    expect(last['role'], 'user');
    expect((last['content']! as List).first, {
      'type': 'text',
      'text': finishNudge,
    });
  });

  test('a turn cut off, or refused, fails', () async {
    final agent = agentOn();
    backend.answer(const [], stopReason: 'max_tokens');
    await expectLater(
      agent.turn('j', const TurnInput(turn: 1, input: job, base: 0), Future<void>.value()),
      throwsA(isA<AgentError>()),
    );
    backend.answer(const [], stopReason: 'refusal');
    await expectLater(
      agent.turn('j', const TurnInput(turn: 1, input: job, base: 0), Future<void>.value()),
      throwsA(isA<AgentError>()),
    );
  });

  test('a refused tool call is an error result with the tool\'s words', () async {
    final agent = agentOn();
    final r = await agent.tool(
      'j',
      1,
      const OtherCall('x', 'read_file', args: {'path': 'missing.html'}),
      Future<void>.value(),
    );
    expect(r.isError, isTrue);
    expect((r.data! as Map)['error'], isA<String>());
  });

  group('give_up', () {
    test('fails the turn with its reason, and runs nothing', () async {
      final agent = agentOn();
      backend.answer([
        _use('w', 'write_file', {'path': 'index.html', 'content': page}),
        _use('g', 'give_up', {'reason': 'It cannot be done.'}),
      ]);
      await expectLater(
        agent.turn('j', const TurnInput(turn: 1, input: job, base: 0), Future<void>.value()),
        throwsA(
          isA<AgentError>().having((e) => e.message, 'message', 'It cannot be done.'),
        ),
      );
      expect(served, isEmpty);
    });

    test('out of room is a stop at the limit most nearly spent', () async {
      final agent = agentOn(capUsd: 6);
      backend.answer([
        _use('g', 'give_up', {'reason': 'Half done.', 'out_of_room': true}),
      ]);
      final out = await agent.turn(
        'j',
        const TurnInput(turn: 1, input: job, base: 0),
        Future<void>.value(),
      );
      expect(out.notes.calls, isEmpty);
      expect(out.stop?.canKeepGoing, isTrue);
      expect((out.stop!.data! as Map)['turn'], 1);
      expect((out.stop!.data! as Map)['notes'], contains('Half done.'));
    });
  });

  group('the limits', () {
    const oneTurn = RunCaps(
      turns: 1,
      renders: 3,
      outputTokens: 32000,
      wallClock: Duration(seconds: 90),
    );

    test('at a cap with a clean render, the stop publishes that render, '
        'asking the model nothing', () async {
      final agent = agentOn(caps: oneTurn, capUsd: 6);
      backend.answer([
        {'type': 'text', 'text': 'Building the list.'},
        _use('w', 'write_file', {'path': 'index.html', 'content': page}),
        _use('m', 'set_meta', {'title': 'Todo'}),
        _use('r', 'render', {}),
        _use('w2', 'write_file', {'path': 'app.js', 'content': 'later'}),
      ]);
      final one = await agent.turn(
        'j',
        const TurnInput(turn: 1, input: job, base: 0),
        Future<void>.value(),
      );
      final results = await runCalls(agent, 1, one.notes);
      final two = await agent.turn(
        'j',
        TurnInput(turn: 2, input: job, base: 0, previous: one.notes, results: results),
        Future<void>.value(),
      );
      expect(backend.requests, hasLength(1));
      expect(two.stop, isNull);
      final stop = two.notes.calls.single as FinishCall;
      final request = (stop.publish.data! as Map)['request'] as Map;
      expect(request['files'], {
        'index.html': {'content': page},
      }, reason: 'the clean render\'s snapshot, not the write after it');
      expect(request['note'], contains('I stopped early (1 turns)'));
      expect(
        (stop.publish.data! as Map)['draft'],
        {'id': 'd1', 'digest': draftDigest(const {'index.html': page})},
      );
      // The plan is whole: published as it is.
      expect(await agent.publishPlan('j', 2, stop.id, stop.publish), stop.publish);
    });

    test('at a cap with nothing clean, the job stops; Keep going only while '
        'its budget lasts', () async {
      for (final (capUsd, can) in [(6.0, true), (0.01, false), (null, false)]) {
        final agent = agentOn(caps: oneTurn, capUsd: capUsd);
        backend.answer([
          _use('w', 'write_file', {'path': 'index.html', 'content': page}),
        ]);
        final one = await agent.turn(
          'j',
          const TurnInput(turn: 1, input: job, base: 0),
          Future<void>.value(),
        );
        final results = await runCalls(agent, 1, one.notes);
        final two = await agent.turn(
          'j',
          TurnInput(turn: 2, input: job, base: 0, previous: one.notes, results: results),
          Future<void>.value(),
        );
        expect(two.stop?.canKeepGoing, can, reason: 'cap \$$capUsd');
        expect((two.stop!.data! as Map)['cap'], '1 turns');
        expect((two.stop!.data! as Map)['turn'], 2);
      }
    });

    test('Keep going is a new run: its own first message and budget, told to '
        'plan the rest when the row says so', () async {
      final agent = agentOn(caps: oneTurn, capUsd: 6);
      backend.answer([
        _use('w', 'write_file', {'path': 'index.html', 'content': page}),
      ]);
      final one = await agent.turn(
        'j',
        const TurnInput(turn: 1, input: job, base: 0),
        Future<void>.value(),
      );
      final results = await runCalls(agent, 1, one.notes);
      final stop = (await agent.turn(
        'j',
        TurnInput(turn: 2, input: job, base: 0, previous: one.notes, results: results),
        Future<void>.value(),
      )).stop!;
      final carry = Carry(stops: 2, canKeepGoing: true, data: stop.data);
      backend.answer([_use('f', 'finish', {'reply': 'Done.'})]);
      final again = await agent.turn(
        'j',
        TurnInput(
          turn: 2,
          input: job,
          base: 0,
          previous: one.notes,
          results: results,
          carry: carry,
          planRest: true,
        ),
        Future<void>.value(),
      );
      expect(again.notes.calls.single, isA<FinishCall>());
      final sent = backend.requests.last.messages;
      expect(sent, hasLength(1), reason: 'a fresh conversation');
      expect(sent.single['content'], [
        {'type': 'text', 'text': 'keep going over 1'},
      ]);
      expect(notes.single.stops, 2);
      expect(notes.single.limit, '1 turns');
      expect(notes.single.edited, ['index.html']);
    });
  });

  group('rebuilt from the rows', () {
    final t1 = TurnNotes(
      calls: const [
        OtherCall('w', 'write_file', args: {'path': 'index.html', 'content': page}),
      ],
      data: {
        'assistant': [
          _use('w', 'write_file', {'path': 'index.html', 'content': page}),
        ],
        'usage': {'input': 10, 'cache_write': 0, 'cache_read': 0, 'output': 100},
      },
    );
    final t2 = TurnNotes(
      calls: const [
        OtherCall('a', 'write_file', args: {'path': 'app.js', 'content': 'x'}),
        RenderCall('r', RenderSpec(draftId: '', revision: 0)),
      ],
      data: {
        'assistant': [
          _use('a', 'write_file', {'path': 'app.js', 'content': 'x'}),
          _use('r', 'render', {}),
        ],
        'usage': {'input': 10, 'cache_write': 0, 'cache_read': 0, 'output': 200},
      },
    );
    final row = Job(
      id: 'j',
      kind: JobKind.make,
      state: JobState.running,
      step: const TurnKey(2),
      rev: 5,
      input: job,
      steps: {
        const TurnKey(1): TurnStepResult(t1),
        const TurnKey(2): TurnStepResult(t2),
      },
    );

    test('a worker that opens a job at turn 3 holds every earlier turn\'s '
        'writes, the run\'s history and its spend', () async {
      final agent = agentOn(row: row);
      // The queue runs turn 2's calls again (their results are not
      // durable): its write, then its render.
      await agent.tool('j', 2, t2.calls.first as OtherCall, Future<void>.value());
      await agent.renderSpec('j', 2, 'r', (t2.calls[1] as RenderCall).spec);
      expect(
        served.single.$2.files,
        {'index.html': page, 'app.js': 'x'},
        reason: 'turn 1\'s write was run again before turn 2\'s',
      );
      backend.answer([_use('f', 'finish', {'reply': 'Done.'})]);
      await agent.turn(
        'j',
        TurnInput(
          turn: 3,
          input: job,
          base: 0,
          previous: t2,
          results: const {'a': ToolResult(data: {'ok': true})},
        ),
        Future<void>.value(),
      );
      final r = backend.requests.single;
      expect(r.messages.map((m) => m['role']), [
        'user',
        'assistant',
        'user',
        'assistant',
        'user',
      ]);
      expect(r.outputBefore, 300, reason: 'the budget counts the rows\' turns');
      final last = r.messages.last['content']! as List;
      expect((last[1] as Map)['is_error'], isTrue, reason: 'a call with no result');
      expect(opened, 1);
    });

    test('a fresh worker at the same rows asks the same turn', () async {
      Future<TurnRequest> fresh() async {
        backend = _Turns()..answer([_use('f', 'finish', {'reply': 'Done.'})]);
        served = [];
        final agent = agentOn(row: row);
        await agent.tool('j', 2, t2.calls.first as OtherCall, Future<void>.value());
        await agent.turn(
          'j',
          TurnInput(turn: 3, input: job, base: 0, previous: t2),
          Future<void>.value(),
        );
        return backend.requests.single;
      }

      final a = await fresh();
      final b = await fresh();
      expect(jsonEncode(b.messages), jsonEncode(a.messages));
      expect(b.outputBefore, a.outputBefore);
    });
  });

  group('renders kept in the rows', () {
    const titled = {'index.html': page};
    TurnNotes turn(
      List<ToolCall> calls,
      List<Map<String, Object?>> uses, {
      List<Map<String, Object?>>? rendered,
    }) => TurnNotes(
      calls: calls,
      data: {
        'assistant': uses,
        'usage': {'input': 10, 'cache_write': 0, 'cache_read': 0, 'output': 10},
        'rendered': ?rendered,
      },
    );
    final t1 = turn(
      const [
        OtherCall('w', 'write_file', args: {'path': 'index.html', 'content': page}),
        OtherCall('m', 'set_meta', args: {'title': 'Todo'}),
      ],
      [
        _use('w', 'write_file', {'path': 'index.html', 'content': page}),
        _use('m', 'set_meta', {'title': 'Todo'}),
      ],
    );
    final t2 = turn(
      const [RenderCall('r', RenderSpec(draftId: '', revision: 0))],
      [_use('r', 'render', {})],
    );

    /// Turn 2's render as turn 3's notes keep it, drawn over [files].
    Map<String, Object?> kept({
      Map<String, String> files = titled,
      List<String> errors = const [],
    }) => {
      'turn': 2,
      'call': 'r',
      'draft_id': 'd9',
      'digest': draftDigest(files),
      'errors': errors,
      'failed_tests': const <String>[],
    };

    Job rowOf(TurnNotes t3) => Job(
      id: 'j',
      kind: JobKind.make,
      state: JobState.running,
      step: const TurnKey(3),
      rev: 6,
      input: job,
      steps: {
        const TurnKey(1): TurnStepResult(t1),
        const TurnKey(2): TurnStepResult(t2),
        const TurnKey(3): TurnStepResult(t3),
      },
    );

    TurnNotes finishTurn(Map<String, Object?>? rendered) => turn(
      const [
        FinishCall(
          'f',
          PublishPlan(
            base: 0,
            data: {
              'finish': {'reply': 'Made it.'},
            },
          ),
        ),
      ],
      [
        _use('f', 'finish', {'reply': 'Made it.'}),
      ],
      rendered: rendered == null ? null : [rendered],
    );

    test('a turn\'s notes keep what the renders of the turn before drew, as '
        'the runner recorded it', () async {
      final agent = agentOn();
      backend
        ..answer([
          _use('w', 'write_file', {'path': 'index.html', 'content': page}),
          _use('m', 'set_meta', {'title': 'Todo'}),
        ])
        ..answer([_use('r', 'render', {})])
        ..answer([_use('f', 'finish', {'reply': 'Made it.'})]);
      final one = await agent.turn(
        'j',
        const TurnInput(turn: 1, input: job, base: 0),
        Future<void>.value(),
      );
      final r1 = await runCalls(agent, 1, one.notes);
      final two = await agent.turn(
        'j',
        TurnInput(turn: 2, input: job, base: 0, previous: one.notes, results: r1),
        Future<void>.value(),
      );
      expect((two.notes.data! as Map)['rendered'], isNull,
          reason: 'turn 1 rendered nothing');
      final r2 = await runCalls(agent, 2, two.notes, dirty: true);
      final three = await agent.turn(
        'j',
        TurnInput(turn: 3, input: job, base: 0, previous: two.notes, results: r2),
        Future<void>.value(),
      );
      expect((three.notes.data! as Map)['rendered'], [
        {
          'turn': 2,
          'call': 'r',
          'draft_id': 'd1',
          'digest': draftDigest(titled),
          'errors': ['boom'],
          'failed_tests': <String>[],
        },
      ]);
    });

    test('a worker killed after a clean render and before the publish\'s '
        'commit publishes the kept render, rendering nothing again', () async {
      Future<Object?> fresh() async {
        served = [];
        final t3 = finishTurn(kept());
        final agent = agentOn(row: rowOf(t3));
        final call = t3.calls.single as FinishCall;
        final plan = await agent.publishPlan('j', 3, 'f', call.publish);
        expect(served, isEmpty, reason: 'nothing is served again');
        return plan.data;
      }

      final a = await fresh();
      expect((a! as Map)['draft'], {'id': 'd9', 'digest': draftDigest(titled)});
      expect(((a as Map)['request'] as Map)['files'], {
        'index.html': {'content': page},
      });
      expect(jsonEncode(await fresh()), jsonEncode(a),
          reason: 'a fresh worker over the same rows plans the same publish');
    });

    test('a kept render is not trusted when the rebuilt draft differs, nor '
        'when it was not clean', () async {
      for (final (entry, code) in [
        (null, 'not_rendered'),
        (kept(files: const {'index.html': '$page '}), 'not_rendered'),
        (kept(errors: const ['boom']), 'render_errors'),
      ]) {
        final agent = agentOn(row: rowOf(finishTurn(entry)));
        await expectLater(
          agent.publishPlan(
            'j',
            3,
            'f',
            const PublishPlan(
              base: 0,
              data: {
                'finish': {'reply': 'Made it.'},
              },
            ),
          ),
          throwsA(isA<PublishRefused>().having((e) => e.code, 'code', code)),
          reason: '$entry',
        );
      }
    });

    test('a cap stop after a restart publishes the last clean render the '
        'rows keep', () async {
      const threeTurns = RunCaps(
        turns: 3,
        renders: 3,
        outputTokens: 32000,
        wallClock: Duration(seconds: 90),
      );
      final t3 = turn(
        const [
          OtherCall('w2', 'write_file', args: {'path': 'app.js', 'content': 'later'}),
        ],
        [
          _use('w2', 'write_file', {'path': 'app.js', 'content': 'later'}),
        ],
        rendered: [kept()],
      );
      Future<Object?> fresh() async {
        backend = _Turns();
        served = [];
        final agent = agentOn(row: rowOf(t3), caps: threeTurns, capUsd: 6);
        // The queue runs turn 3's call again, then starts turn 4.
        final w2 = await agent.tool(
          'j',
          3,
          t3.calls.single as OtherCall,
          Future<void>.value(),
        );
        final four = await agent.turn(
          'j',
          TurnInput(
            turn: 4,
            input: job,
            base: 0,
            previous: t3,
            results: {'w2': w2},
          ),
          Future<void>.value(),
        );
        expect(backend.requests, isEmpty, reason: 'the model is asked nothing');
        expect(four.stop, isNull, reason: 'the kept render is published');
        return (four.notes.calls.single as FinishCall).publish.data;
      }

      final a = await fresh();
      expect(((a! as Map)['request'] as Map)['files'], {
        'index.html': {'content': page},
      }, reason: 'the clean render\'s snapshot, not the write after it');
      expect((a as Map)['draft'], {'id': 'd9', 'digest': draftDigest(titled)});
      expect(jsonEncode(await fresh()), jsonEncode(a));
    });
  });
}

Map<String, Object?> _use(String id, String name, Map<String, Object?> input) =>
    {'type': 'tool_use', 'id': id, 'name': name, 'input': input};

/// A model that answers each turn from a script; a turn with none
/// scripted fails the test.
final class _Turns implements TurnBackend {
  final List<ModelTurn> _answers = [];
  final List<TurnRequest> requests = [];

  void answer(List<Map<String, Object?>> content, {String stopReason = 'tool_use'}) =>
      _answers.add(
        ModelTurn(content: content, stopReason: stopReason, usage: const Usage()),
      );

  @override
  String get name => 'scripted';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    requests.add(r);
    if (_answers.isEmpty) throw StateError('no turn scripted');
    return _answers.removeAt(0);
  }
}
