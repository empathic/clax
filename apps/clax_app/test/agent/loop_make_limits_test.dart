import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/unfinished.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const key = 'new:k1';

/// A new app's first run at a limit: the draft is kept under its plan key
/// and the make ends [MakeStopped], not failed; Keep going runs on from
/// the kept draft within the build's $6, from the second stop it plans
/// the rest, and a clean render at the stop makes the app for the build
/// to finish.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late HeldRunner runner;
  late MemoryUnfinishedChanges unfinished;
  late PlanStore plans;
  late String sid;

  AgentLoop loopWith(List<List<ScriptedCall>> runs) {
    runner = HeldRunner(runs);
    return AgentLoop(
      api: api,
      runner: () => runner,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
      inspector: FakeInspector(),
      baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
      plans: plans,
      unfinished: unfinished,
      clock: StepClock(),
    );
  }

  Map<String, Object?> f(String id, String title, [String status = 'todo']) =>
      {'id': id, 'title': title, 'detail': '', 'status': status};

  /// Plans, writes a page it never renders, then gives up out of room.
  List<ScriptedCall> outOfRoom({String page = '<p>normal mode</p>'}) => [
    ScriptedCall('plan_set', {
      'features': [f('f1', 'Modal editing'), f('f2', 'Save and quit')],
    }, 'Planning the editor'),
    const ScriptedCall('set_meta', {
      'title': 'Vim',
      'capabilities_json': null,
    }),
    ScriptedCall('write_file', {'path': 'index.html', 'content': page},
        'Writing the modal editor'),
    const ScriptedCall('give_up', {
      'reason': 'I wrote the file list and a modal editor, but ran out of '
          'room before I could get a clean render.',
      'out_of_room': true,
    }),
  ];

  setUp(() async {
    api = FakeClaxApi();
    unfinished = MemoryUnfinishedChanges();
    plans = PlanStore();
    sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
  });

  test('a give_up out of room with work done ends MakeStopped, keeps the '
      'draft and the plan under the plan key, and makes nothing', () async {
    final loop = loopWith([outOfRoom()]);
    final err = await loop
        .newApp('A vim inspired text editor', session: sid, planKey: key)
        .then<Object?>((_) => null, onError: (Object e) => e);

    expect(err, isA<MakeStopped>());
    final s = err! as MakeStopped;
    expect(s.key, key);
    expect(s.stops, 1);
    expect(s.planned, isFalse);
    expect(s.canKeepGoing, isTrue);
    expect(s.capUsd, buildCapUsd);
    expect(s.changed.single.path, 'index.html');
    expect(s.notes.last, contains('ran out of room'));
    expect(s.html, '<p>normal mode</p>');
    expect(await api.listArtifacts(), isEmpty);

    final kept = (await unfinished.get(key))!;
    expect(kept.stops, 1);
    expect(kept.edits['index.html'], '<p>normal mode</p>');
    expect(kept.title, 'Vim');
    expect(kept.notes, [
      'Planning the editor',
      'Writing the modal editor',
      startsWith('I wrote the file list'),
    ]);
    expect((await plans.get(key))!.features, hasLength(2));
  });

  test('a cap with no clean render ends MakeStopped at that cap', () async {
    final loop = loopWith([
      [
        const ScriptedCall('write_file', {
          'path': 'index.html',
          'content': '<p>x</p>',
        }),
        for (var i = 0; i < 20; i++) const ScriptedCall('list_files', {}),
      ],
    ]);
    final err = await loop
        .newApp('x', session: sid, planKey: key)
        .then<Object?>((_) => null, onError: (Object e) => e);
    expect(err, isA<MakeStopped>());
    expect((err! as MakeStopped).limit, '${RunCaps.first.turns} turns');
    expect((await unfinished.get(key))!.limit, '${RunCaps.first.turns} turns');
  });

  test('a give_up without out_of_room, or with nothing written, still '
      'fails plainly and keeps nothing', () async {
    final loop = loopWith([
      [
        const ScriptedCall('write_file', {
          'path': 'index.html',
          'content': '<p>x</p>',
        }),
        const ScriptedCall('give_up', {'reason': 'it needs a camera'}),
      ],
      [
        const ScriptedCall('give_up', {
          'reason': 'no room',
          'out_of_room': true,
        }),
      ],
    ]);
    for (var i = 0; i < 2; i++) {
      final err = await loop
          .newApp('x', session: sid, planKey: key)
          .then<Object?>((_) => null, onError: (Object e) => e);
      expect(err, isA<AgentError>(), reason: 'run $i');
      expect(err, isNot(isA<MakeStopped>()));
    }
    expect(await unfinished.get(key), isNull);
  });

  test('Keep going starts from the kept draft with the notes, the count and '
      'what is left of the build\'s budget, and a finish makes the app from '
      'it', () async {
    final loop = loopWith([
      outOfRoom(),
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Made it.', 'resolve': false}),
      ],
    ]);
    await expectLater(
      loop.newApp('A vim editor', session: sid, planKey: key),
      throwsA(isA<MakeStopped>()),
    );
    // What the stopped run spent, as the plan and the kept draft count it.
    final k = (await unfinished.get(key))!;
    await unfinished.put(
      UnfinishedChange(
        artifactId: k.artifactId,
        threadIds: k.threadIds,
        commentIds: k.commentIds,
        limit: k.limit,
        spentUsd: 5.25,
        at: k.at,
        stops: k.stops,
        notes: k.notes,
        edits: k.edits,
        before: k.before,
        title: k.title,
      ),
    );

    final aid = await loop.newApp(
      'A vim editor',
      session: sid,
      planKey: key,
      keepGoing: true,
    );

    expect(runner.states[1].draft.files['index.html'], '<p>normal mode</p>');
    expect(runner.states[1].draft.title, 'Vim');
    expect(runner.states[1].budget.caps.costUsd, closeTo(0.75, 1e-9));
    final text = runner.text(1);
    expect(text, contains('stopped at its limit once'));
    expect(text, contains('"Writing the modal editor"'));
    expect(text, contains('<p>normal mode</p>'));
    expect(text, isNot(contains('An earlier attempt made this plan')));
    expect(api.filesOf(aid, 1)['index.html'], '<p>normal mode</p>');
    expect(await unfinished.get(key), isNull, reason: 'done with');
    expect(await plans.get(key), isNull);
    expect((await plans.get(aid))!.features, hasLength(2));
  });

  test('a Keep going that stops again counts both stops, and the run after '
      'the second stop is told to plan the rest and finish what works',
      () async {
    final loop = loopWith([
      outOfRoom(),
      outOfRoom(page: '<p>normal and insert</p>'),
      outOfRoom(page: '<p>all modes</p>'),
    ]);
    Future<MakeStopped> stop({bool keepGoing = true}) async =>
        await loop
                .newApp('x', session: sid, planKey: key, keepGoing: keepGoing)
                .then<Object?>((_) => null, onError: (Object e) => e)
            as MakeStopped;

    final first = await stop(keepGoing: false);
    expect(first.stops, 1);
    final second = await stop();
    expect(second.stops, 2);
    expect(second.planned, isFalse);
    expect(runner.text(1), isNot(contains('too big for one run')));
    final third = await stop();
    expect(third.stops, 3);
    expect(third.planned, isTrue, reason: 'it was told to plan the rest');
    final text = runner.text(2);
    expect(text, contains('stopped at its limit twice'));
    expect(text, contains('too big for one run'));
    expect(text, contains('plan_set'));
    expect((await unfinished.get(key))!.notes, hasLength(9));
  });

  test('a Keep going with nothing left of the budget cannot go on', () async {
    final loop = loopWith([outOfRoom()]);
    await unfinished.put(
      UnfinishedChange(
        artifactId: key,
        threadIds: const [key],
        commentIds: const [],
        limit: '14 turns',
        spentUsd: 5.99,
        at: DateTime.utc(2026, 10, 19),
        edits: const {'index.html': '<p>x</p>'},
        before: const {'index.html': null},
      ),
    );
    final err = await loop
        .newApp('x', session: sid, planKey: key, keepGoing: true)
        .then<Object?>((_) => null, onError: (Object e) => e);
    expect((err! as MakeStopped).canKeepGoing, isFalse);
  });

  test('a give_up out of room after a clean render makes the app from that '
      'render, and the rest of the draft goes to the build', () async {
    final loop = loopWith([
      [
        ScriptedCall('plan_set', {
          'features': [f('f1', 'Modal editing'), f('f2', 'Save and quit')],
        }),
        ...newAppCalls(page: '<p>shell</p>').take(3),
        const ScriptedCall('write_file', {
          'path': 'index.html',
          'content': '<p>shell and modes</p>',
        }),
        const ScriptedCall('give_up', {
          'reason': 'ran out of room',
          'out_of_room': true,
        }),
      ],
    ]);
    final aid = await loop.newApp('x', session: sid, planKey: key);
    expect(api.filesOf(aid, 1)['index.html'], '<p>shell</p>');
    final carried = (await unfinished.get(buildDraftKey(aid)))!;
    expect(carried.edits['index.html'], '<p>shell and modes</p>');
    expect(carried.before['index.html'], fingerprint('<p>shell</p>'));
    expect((await plans.get(aid))!.todo, isNotEmpty);
    expect(await unfinished.get(key), isNull);
  });

  test('a cap stop after a clean render keeps the rest of the draft for the '
      'build', () async {
    final loop = loopWith([
      [
        ScriptedCall('plan_set', {
          'features': [f('f1', 'Modal editing')],
        }),
        ...newAppCalls(page: '<p>shell</p>').take(3),
        const ScriptedCall('write_file', {
          'path': 'index.html',
          'content': '<p>more</p>',
        }),
        for (var i = 0; i < 20; i++) const ScriptedCall('list_files', {}),
      ],
    ]);
    final aid = await loop.newApp('x', session: sid, planKey: key);
    expect(api.filesOf(aid, 1)['index.html'], '<p>shell</p>');
    expect(
      (await unfinished.get(buildDraftKey(aid)))!.edits['index.html'],
      '<p>more</p>',
    );
  });
}
