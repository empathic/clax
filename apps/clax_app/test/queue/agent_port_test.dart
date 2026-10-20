import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/messages_runner.dart' show finishNudge;
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/queue/agent_port.dart';
import 'package:clax_app/queue/model.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../agent/tools/support.dart';

/// The queue's agent over lib/agent's parts: one model turn per `turn`, the
/// job's toolbox per `tool`, and the draft served or published only when
/// the queue runs the call, after the calls before it.
void main() {
  late _Turns backend;
  late List<(JobId, DraftSnapshot)> served;
  late int opened;

  AgentTurns agentOn(Map<String, String> files, {RequestKind? kind}) {
    return AgentTurns(
      backend: backend,
      open: (jobId) async {
        opened++;
        return AgentJob(
          input: const RunInput(
            system: 'You make apps.',
            first: [
              {'type': 'text', 'text': 'a todo app'},
            ],
          ),
          toolbox: toolboxWith(
            files: files,
            kind: kind ?? RequestKind.newApp,
            title: null,
            baseVersion: 0,
          ),
        );
      },
      serveDraft: (jobId, snap) async {
        served.add((jobId, snap));
        return 'd${served.length}';
      },
    );
  }

  setUp(() {
    backend = _Turns();
    served = [];
    opened = 0;
  });

  const job = JobInput(request: 'a todo app');
  const page = '<title>Todo</title><p>todo</p>';

  test('a turn\'s calls are the queue\'s, and the draft is served and '
      'published as the calls before them left it', () async {
    final agent = agentOn(const {});
    backend.answer([
      {'type': 'text', 'text': 'Writing it.'},
      _use('w', 'write_file', {'path': 'index.html', 'content': page}),
      _use('t', 'write_file', {
        'path': 'tests/a.test.js',
        'content': 'test("a", () => {})',
      }),
      _use('r', 'render', {}),
      _use('f', 'finish', {'reply': 'Made it.'}),
    ]);
    final out = await agent.turn(
      'j',
      const TurnInput(turn: 1, input: job, base: 0),
      Future<void>.value(),
    );
    final calls = out.notes.calls;
    expect(calls.map((c) => c.id), ['w', 't', 'r', 'f']);
    expect(calls[0], isA<OtherCall>().having((c) => c.name, 'name', 'write_file'));
    expect(calls[2], isA<RenderCall>());
    expect(
      calls[3],
      const FinishCall('f', PublishPlan(base: 0)),
      reason: 'a make replies on no threads',
    );
    expect(served, isEmpty, reason: 'nothing is served before the call runs');

    // The queue runs the calls in order.
    for (final c in calls.take(2)) {
      final r = await agent.tool('j', 1, c as OtherCall, Future<void>.value());
      expect(r.isError, isFalse, reason: '$r');
    }
    final spec = await agent.renderSpec(
      'j',
      1,
      'r',
      (calls[2] as RenderCall).spec,
    );
    final files = {'index.html': page, 'tests/a.test.js': 'test("a", () => {})'};
    expect(served.single.$2.files, files);
    expect(
      spec,
      RenderSpec(
        draftId: 'd1',
        revision: served.single.$2.revision,
        digest: draftDigest(files),
        files: const ['tests/a.test.js'],
      ),
    );
    expect(spec.digest, startsWith('sha256:'));

    final plan = await agent.publishPlan(
      'j',
      1,
      'f',
      (calls[3] as FinishCall).publish,
    );
    expect(plan.base, 0);
    expect(plan.data, {
      'artifact_id': null,
      'request': {
        'title': null,
        'files': {
          'index.html': {'content': page},
          'tests/a.test.js': {'content': 'test("a", () => {})'},
        },
      },
      'draft': {'id': 'd1', 'digest': draftDigest(files)},
    });
    expect(opened, 1);
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

  test('a change\'s finish carries its reply on the job\'s threads', () async {
    final agent = agentOn(const {'index.html': page}, kind: RequestKind.change);
    backend.answer([
      _use('f', 'finish', {'reply': 'Done.', 'resolve': true}),
    ]);
    final out = await agent.turn(
      'j',
      const TurnInput(
        turn: 1,
        input: JobInput(request: 'x', base: 3, threads: {'t1': 'c1'}),
        base: 3,
      ),
      Future<void>.value(),
    );
    expect(
      out.notes.calls.single,
      const FinishCall(
        'f',
        PublishPlan(base: 3),
        reply: ReplyPlan(
          data: {
            'threads': ['t1'],
            'body': 'Done.',
            'resolve': true,
          },
        ),
      ),
    );
  });

  test('the next turn sends the history: the turn before and its calls\' '
      'results in order, a render\'s screenshot as an image', () async {
    final agent = agentOn(const {});
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

  test('a worker that starts at a later turn rebuilds the history from the '
      'first message and the turn before', () async {
    final agent = agentOn(const {});
    backend.answer([_use('f', 'finish', {})]);
    final before = TurnNotes(
      calls: const [OtherCall('w', 'list_files')],
      data: {
        'assistant': [
          _use('w', 'list_files', {}),
        ],
      },
    );
    await agent.turn(
      'j',
      TurnInput(
        turn: 4,
        input: job,
        base: 0,
        previous: before,
        results: const {},
      ),
      Future<void>.value(),
    );
    final sent = backend.requests.single.messages;
    expect(sent.map((m) => m['role']), ['user', 'assistant', 'user']);
    expect(
      ((sent[2]['content']! as List).single as Map)['is_error'],
      isTrue,
      reason: 'a call with no result answers as an error',
    );
  });

  test('a turn that called no tool is followed by the nudge', () async {
    final agent = agentOn(const {});
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
    expect(backend.requests.last.messages.last, {
      'role': 'user',
      'content': [
        {'type': 'text', 'text': finishNudge},
      ],
    });
  });

  test('a turn cut off, or refused, fails', () async {
    final agent = agentOn(const {});
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
    final agent = agentOn(const {});
    final r = await agent.tool(
      'j',
      1,
      const OtherCall('x', 'read_file', args: {'path': 'missing.html'}),
      Future<void>.value(),
    );
    expect(r.isError, isTrue);
    expect((r.data! as Map)['error'], isA<String>());
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
