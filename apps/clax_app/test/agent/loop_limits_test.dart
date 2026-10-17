import 'dart:io';

import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/unfinished.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/clax/models.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const page = '<!doctype html><title>Runs</title><button>Save</button>';

/// A change at a limit: Clax's reply naming it, the change kept for Keep
/// going, and Keep going running on from the kept draft within the
/// change's $6.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late HeldRunner runner;
  late MemoryUnfinishedChanges unfinished;
  late PlanStore plans;
  late StepClock clock;
  late FakeInspector inspector;
  late String sid;
  late String aid;

  AgentLoop loopWith(List<List<ScriptedCall>> runs) {
    runner = HeldRunner(runs);
    return AgentLoop(
      api: api,
      runner: () => runner,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
      inspector: inspector,
      baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
      plans: plans,
      unfinished: unfinished,
      clock: clock,
    );
  }

  List<String> bodies(String tid) => [
    for (final c in api.threadOf(aid, tid).comments) c.body,
  ];

  /// An edit to the page that is never rendered, then turns spent until
  /// the 10-turn cap stops the run.
  List<ScriptedCall> unfinishedRun() => [
    const ScriptedCall('str_replace', {
      'path': 'index.html',
      'old_str': 'Save',
      'new_str': 'Keep',
    }, 'Renaming the button'),
    for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
  ];

  setUp(() async {
    api = FakeClaxApi();
    unfinished = MemoryUnfinishedChanges();
    plans = PlanStore();
    clock = StepClock();
    inspector = FakeInspector();
    sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
    aid = api.seedArtifact('Runs', {'index.html': page});
  });

  test(
    'a cap with nothing to publish replies in Clax\'s words naming the '
    'limit, with the agent\'s words under it, and keeps the change',
    () async {
      final item = api.sendComment(aid, 'Rename Save', queue: false);
      final out = await loopWith([unfinishedRun()])
          .change(aid, [item], session: sid);

      expect(out, isA<ChangeFailed>());
      final reply = bodies(item.threadId).last;
      expect(
        reply,
        "Couldn't make that change: it was too big for one go — it needed "
        'more than 10 steps. $keepGoingHint\n\n'
        'Stopped once · \$0.00 of \$6 used\n'
        'Changed so far: index.html (+1 −1)\n\n'
        "The agent's latest notes:\n"
        '- Renaming the button',
      );
      expect(isFailureText(reply), isTrue);
      expect(offersKeepGoing((out as ChangeFailed).reason), isTrue);
      expect(api.currentVersion(aid), 1);

      final kept = await unfinished.get(item.threadId);
      expect(kept, isNotNull);
      expect(kept!.limit, '10 turns');
      expect(kept.commentIds, [item.commentId]);
      expect(kept.notes, ['Renaming the button']);
      expect(kept.edits['index.html'], contains('<button>Keep</button>'));
      expect(kept.before['index.html'], fingerprint(page));
    },
  );

  test('a give_up out of room names the limit and quotes the agent', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await loopWith([
      [
        for (var i = 0; i < 8; i++) const ScriptedCall('list_files', {}),
        const ScriptedCall('give_up', {
          'reason': 'I ran out of room in this run',
          'out_of_room': true,
        }),
      ],
    ]).change(aid, [item], session: sid);

    expect(
      bodies(item.threadId).last,
      "Couldn't make that change: it was too big for one go — it needed "
      'more than 10 steps. $keepGoingHint\n\n'
      'Stopped once · \$0.00 of \$6 used\n\n'
      "The agent's latest notes:\n"
      '- I ran out of room in this run',
    );
    expect((await unfinished.get(item.threadId))!.notes, [
      'I ran out of room in this run',
    ]);
  });

  test('a give_up near a cap without out_of_room replies the plain failure '
      'and keeps nothing', () async {
    final item = api.sendComment(aid, 'Use the camera', queue: false);
    await loopWith([
      [
        for (var i = 0; i < 8; i++) const ScriptedCall('list_files', {}),
        const ScriptedCall('give_up', {
          'reason': 'it needs the camera',
          'out_of_room': false,
        }),
      ],
    ]).change(aid, [item], session: sid);
    expect(bodies(item.threadId).last, failureReply('it needs the camera'));
    expect(offersKeepGoing(bodies(item.threadId).last), isFalse);
    expect(await unfinished.get(item.threadId), isNull);
  });

  test('a give_up far from every cap replies as before and keeps '
      'nothing', () async {
    final item = api.sendComment(aid, 'Use the camera', queue: false);
    await loopWith([
      [
        const ScriptedCall('give_up', {'reason': 'it needs a camera'}),
      ],
    ]).change(aid, [item], session: sid);
    expect(bodies(item.threadId).last, failureReply('it needs a camera'));
    expect(await unfinished.get(item.threadId), isNull);
  });

  test('a cap after a clean render publishes it, says in Clax\'s words it '
      'is only part, and keeps what the draft held beyond it', () async {
    final item = api.sendComment(aid, 'Rename and recolour', queue: false);
    final out = await loopWith([
      [
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': 'Save',
          'new_str': 'Keep',
        }, 'Renaming the button'),
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': '<button>',
          'new_str': '<button class="red">',
        }),
        for (var i = 0; i < 8; i++) const ScriptedCall('list_files', {}),
      ],
    ]).change(aid, [item], session: sid);

    expect(out, isA<ChangeLanded>());
    expect(
      bodies(item.threadId).last,
      'This change was too big for one go — it needed more than 10 steps — '
      'so this version has only part of it. $keepGoingHint\n\n'
      'Stopped once · \$0.00 of \$6 used\n'
      'Changed so far: index.html (+1 −1)\n\n'
      "The agent's latest notes:\n"
      '- Renaming the button',
    );
    final kept = (await unfinished.get(item.threadId))!;
    expect(
      kept.before['index.html'],
      fingerprint(api.filesOf(aid, 2)['index.html']),
    );
    expect(kept.edits['index.html'], contains('class="red"'));
  });

  test('Keep going after a partial publish starts from the kept edits on '
      'the published version', () async {
    final item = api.sendComment(aid, 'Rename and recolour', queue: false);
    final loop = loopWith([
      [
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': 'Save',
          'new_str': 'Keep',
        }),
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': '<button>',
          'new_str': '<button class="red">',
        }),
        for (var i = 0; i < 8; i++) const ScriptedCall('list_files', {}),
      ],
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Recoloured.', 'resolve': true}),
      ],
    ]);
    await loop.change(aid, [item], session: sid);
    expect(api.currentVersion(aid), 2);
    await loop.change(aid, [item], session: sid, keepGoing: true);
    expect(
      runner.states[1].draft.files['index.html'],
      contains('<button class="red">Keep'),
    );
    expect(runner.text(1), contains('already holds that run'));
    expect(api.filesOf(aid, 3)['index.html'], contains('class="red"'));
  });

  test('Keep going on an app that changed under the draft starts over with '
      'the notes', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    final loop = loopWith([
      unfinishedRun(),
      [
        const ScriptedCall('finish', {
          'reply': 'Started over.',
          'resolve': false,
        }),
      ],
    ]);
    await loop.change(aid, [item], session: sid);
    // Another version lands on the edited path meanwhile.
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: api.currentVersion(aid),
        files: {'index.html': '<!doctype html><button>Other</button>'},
      ),
    );
    await loop.change(aid, [item], session: sid, keepGoing: true);
    expect(
      runner.states[1].draft.files['index.html'],
      '<!doctype html><button>Other</button>',
    );
    expect(
      runner.text(1),
      contains('the app changed under its unfinished draft'),
    );
    expect(runner.text(1), contains('"Renaming the button"'));
    expect(bodies(item.threadId).last, '$startedOverLine Started over.');
  });

  test('Keep going starts from the kept draft, tells the agent what the '
      'earlier run said, caps its cost at what is left of the change\'s '
      r'$6, and forgets the change once it finishes', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: [item.commentId],
        limit: '10 turns',
        spentUsd: 5.0,
        at: clock.now(),
        notes: const ['Renaming the button'],
        edits: {'index.html': page.replaceFirst('Save', 'Keep')},
        before: {'index.html': fingerprint(page)},
      ),
    );
    final loop = loopWith([
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Renamed.', 'resolve': true}),
      ],
    ]);
    final out = await loop.change(aid, [item], session: sid, keepGoing: true);

    expect(out, isA<ChangeLanded>());
    expect(api.filesOf(aid, 2)['index.html'], contains('<button>Keep'));
    expect(runner.states.single.budget.caps.costUsd, closeTo(1.0, 1e-9));
    expect(runner.states.single.ctx.changeSpentUsd, 5.0);
    final first = runner.text(0);
    expect(first, contains('Keep going'));
    expect(first, contains('"Renaming the button"'));
    expect(first, contains('already holds that run'));
    expect(first, contains('stage again any the files rely on'));
    expect(await unfinished.get(item.threadId), isNull);
  });

  test('a Keep going that stops again counts both runs and says what is '
      'left', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: [item.commentId],
        limit: '10 turns',
        spentUsd: 4.5,
        at: clock.now(),
      ),
    );
    await loopWith([unfinishedRun()])
        .change(aid, [item], session: sid, keepGoing: true);
    expect(bodies(item.threadId).last, contains(r'About $1.50 of its $6'));
    expect((await unfinished.get(item.threadId))!.spentUsd, 4.5);
  });

  test('each Keep going carries the notes of every earlier run, oldest '
      'first, along with the edits they made', () async {
    final item = api.sendComment(aid, 'Rename and recolour', queue: false);
    List<ScriptedCall> stopsAfter(ScriptedCall edit) => [
      edit,
      for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
    ];
    final loop = loopWith([
      stopsAfter(
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': 'Save',
          'new_str': 'Keep',
        }, 'Renaming the button'),
      ),
      stopsAfter(
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': '<button>',
          'new_str': '<button class="red">',
        }, 'Recolouring the button'),
      ),
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Done.', 'resolve': true}),
      ],
    ]);
    await loop.change(aid, [item], session: sid);
    await loop.change(aid, [item], session: sid, keepGoing: true);
    expect((await unfinished.get(item.threadId))!.notes, [
      'Renaming the button',
      'Recolouring the button',
    ]);
    await loop.change(aid, [item], session: sid, keepGoing: true);

    final third = runner.text(2);
    expect(third, contains('"Renaming the button"'));
    expect(third, contains('"Recolouring the button"'));
    expect(
      third.indexOf('"Renaming the button"'),
      lessThan(third.indexOf('"Recolouring the button"')),
    );
    expect(
      runner.states[2].draft.files['index.html'],
      contains('<button class="red">Keep'),
    );
  });

  test('a second stop says how often the change stopped, what its runs '
      'cost, the files its draft changed and the latest notes of both '
      'runs', () async {
    final item = api.sendComment(aid, 'Rename and recolour', queue: false);
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: [item.commentId],
        limit: '10 turns',
        spentUsd: 1.25,
        at: clock.now(),
        notes: const ['Reading the page', 'Renaming the button'],
        edits: {'index.html': page.replaceFirst('Save', 'Keep')},
        before: {'index.html': fingerprint(page)},
      ),
    );
    await loopWith([
      [
        const ScriptedCall('write_file', {
          'path': 'app.js',
          'content': 'let a = 1;\nlet b = 2;\n',
        }, 'Adding the colour script'),
        for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
      ],
    ]).change(aid, [item], session: sid, keepGoing: true);
    final reply = bodies(item.threadId).last;
    expect(reply, startsWith("Couldn't make that change: it was too big"));
    expect(
      reply,
      endsWith(
        '\n\nStopped twice · \$1.25 of \$6 used\n'
        'Changed so far: app.js (+2), index.html (+1 −1)\n\n'
        "The agent's latest notes:\n"
        '- Reading the page\n'
        '- Renaming the button\n'
        '- Adding the colour script',
      ),
    );
    expect((await unfinished.get(item.threadId))!.stops, 2);
  });

  test('a Keep going is shown its kept edits as a diff against the app and '
      'what the stopped run\'s last render found', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    inspector.answer(
      inspector.clean(
        errors: [(kind: 'error', text: 'TypeError: total is undefined')],
      ),
    );
    final loop = loopWith([
      [
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': 'Save',
          'new_str': 'Keep',
        }),
        const ScriptedCall('render', {'file': null}),
        for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
      ],
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Renamed.', 'resolve': true}),
      ],
    ]);
    await loop.change(aid, [item], session: sid);
    expect(
      (await unfinished.get(item.threadId))!.lastRender,
      contains('TypeError: total is undefined'),
    );
    await loop.change(aid, [item], session: sid, keepGoing: true);
    final first = runner.text(1);
    expect(
      first,
      contains(
        '--- a/index.html\n+++ b/index.html\n@@ -1,1 +1,1 @@\n'
        '-$page\n'
        '+${page.replaceFirst('Save', 'Keep')}\n',
      ),
    );
    expect(first, contains("That run's last render"));
    expect(first, contains('TypeError: total is undefined'));
  });

  test('the notes kept for Keep going are bounded, the newest kept', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: [item.commentId],
        limit: '10 turns',
        spentUsd: 0,
        at: clock.now(),
        notes: [for (var i = 0; i < maxUnfinishedNotes; i++) 'note $i'],
      ),
    );
    await loopWith([unfinishedRun()])
        .change(aid, [item], session: sid, keepGoing: true);
    final notes = (await unfinished.get(item.threadId))!.notes;
    expect(notes, hasLength(maxUnfinishedNotes));
    expect(notes.first, 'note 1');
    expect(notes.last, 'Renaming the button');
  });

  test('an answer the person gave the stopped run is kept and sent to the '
      'Keep going run', () async {
    await api.stream().subscribe(['questions']);
    final item = api.sendComment(aid, 'Show my total', queue: false);
    final loop = loopWith([
      [
        const ScriptedCall('ask', {
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
        }),
        ...unfinishedRun(),
      ],
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'In km.', 'resolve': true}),
      ],
    ]);
    final stopped = loop.change(aid, [item], session: sid);
    await until(() => api.callNames.contains('waitQuestion'));
    final q = (await api.questions()).single;
    await api.answerQuestion(q.id, [
      {
        'selected': ['km'],
        'text': null,
      },
    ]);
    await stopped;
    final kept = (await unfinished.get(item.threadId))!;
    expect(kept.answers, contains('The person answered your question'));
    expect(kept.answers, contains('"km"'));

    await loop.change(aid, [item], session: sid, keepGoing: true);
    final first = runner.text(1);
    expect(first, contains('Answers to the question you asked'));
    expect(first, contains('The person answered your question'));
    expect(first, contains(r'\"km\"'));
  });

  test('a Keep going hours after the stop still starts from the kept '
      'draft', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: [item.commentId],
        limit: '120 seconds',
        spentUsd: 1.5,
        at: clock.now().subtract(const Duration(hours: 5)),
        notes: const ['Renaming the button'],
        edits: {'index.html': page.replaceFirst('Save', 'Keep')},
        before: {'index.html': fingerprint(page)},
      ),
    );
    final loop = loopWith([
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Renamed.', 'resolve': true}),
      ],
    ]);
    await loop.change(aid, [item], session: sid, keepGoing: true);

    expect(
      runner.states.single.draft.files['index.html'],
      contains('<button>Keep'),
    );
    final first = runner.text(0);
    expect(first, contains('already holds that run'));
    expect(first, isNot(contains('started over')));
    expect(api.filesOf(aid, 2)['index.html'], contains('<button>Keep'));
  });

  test('Keep going with a comment the kept change did not answer runs as a '
      'fresh change', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: const ['someone-else'],
        limit: '10 turns',
        spentUsd: 5.9,
        at: clock.now(),
        edits: {'index.html': page.replaceFirst('Save', 'Keep')},
        before: {'index.html': fingerprint(page)},
      ),
    );
    final loop = loopWith([
      [
        const ScriptedCall('finish', {'reply': 'OK.', 'resolve': true}),
      ],
    ]);
    await loop.change(aid, [item], session: sid, keepGoing: true);
    expect(runner.states.single.draft.files['index.html'], page);
    expect(runner.states.single.ctx.changeSpentUsd, 0);
    expect(runner.text(0), isNot(contains('Keep going')));
  });

  test('a run that plans the rest and then stops at a cap says the rest is '
      'being built, offers no Keep going and keeps nothing', () async {
    final item = api.sendComment(aid, 'Add stats', queue: false);
    final out = await loopWith([
      [
        const ScriptedCall('plan_set', {
          'features': [
            {'id': 's1', 'title': 'Stats', 'detail': '', 'status': 'todo'},
          ],
        }, 'Planning the stats'),
        for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
      ],
    ]).change(aid, [item], session: sid);

    // Not a failure: the build goes on with it.
    expect(out, isA<ChangeAnswered>());
    final reply = bodies(item.threadId).last;
    expect(
      reply,
      'This change was too big for one go — it needed more than 10 steps — '
      'so what is left of it is planned and being built in steps: the plan '
      'shows each one as it is built.\n\n'
      'Stopped once · \$0.00 of \$6 used\n\n'
      "The agent's latest notes:\n"
      '- Planning the stats',
    );
    expect(offersKeepGoing(reply), isFalse);
    expect(isFailureText(reply), isFalse);
    expect(await unfinished.get(item.threadId), isNull);
  });

  test(
    'a Keep going that plans the rest and stops before a clean render '
    'carries its draft into the build, whose first run starts from it',
    () async {
      final item = api.sendComment(aid, 'Rename and add stats', queue: false);
      await unfinished.put(
        UnfinishedChange(
          artifactId: aid,
          threadIds: [item.threadId],
          commentIds: [item.commentId],
          limit: '10 turns',
          spentUsd: 2.5,
          stops: 2,
          at: clock.now(),
          edits: {'index.html': page.replaceFirst('Save', 'Keep')},
          before: {'index.html': fingerprint(page)},
        ),
      );
      final loop = loopWith([
        [
          const ScriptedCall('plan_set', {
            'features': [
              {
                'id': 's1',
                'title': 'Stats',
                'detail': 'The draft renames Save already.',
                'status': 'todo',
              },
            ],
          }),
          for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
        ],
        [
          const ScriptedCall('render', {'file': null}),
          const ScriptedCall('plan_mark', {'id': 's1', 'status': 'done'}),
          const ScriptedCall('finish', {'reply': 'Built s1.', 'resolve': true}),
        ],
      ]);
      final out = await loop.change(aid, [item], session: sid, keepGoing: true);
      expect(out, isA<ChangeAnswered>());
      expect(api.currentVersion(aid), 1);
      expect(await unfinished.get(item.threadId), isNull);
      expect(
        (await unfinished.get(buildDraftKey(aid)))!.edits['index.html'],
        contains('<button>Keep'),
      );

      await loop.continueBuild(aid);
      expect(runner.states[1].draft.files['index.html'], contains('Keep'));
      expect(runner.text(1), contains('change that planned these features'));
      expect(api.filesOf(aid, 2)['index.html'], contains('<button>Keep'));
      expect(await unfinished.get(buildDraftKey(aid)), isNull);
    },
  );

  test('a build draft the app changed under is dropped, and the build is '
      'told the features\' notes of it do not hold', () async {
    await plans.set(aid, const [
      PlanFeature(id: 's1', title: 'Stats'),
    ], request: 'runs');
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [buildDraftKey(aid)],
        commentIds: const [],
        limit: '',
        spentUsd: 0,
        at: clock.now(),
        edits: {'index.html': 'stale'},
        before: {'index.html': fingerprint('something else')},
      ),
    );
    await loopWith([
      [
        const ScriptedCall('finish', {
          'reply': 'Nothing yet.',
          'resolve': true,
        }),
      ],
    ]).continueBuild(aid);
    expect(runner.states.single.draft.files['index.html'], page);
    expect(runner.text(0), contains('could not be used'));
    expect(await unfinished.get(buildDraftKey(aid)), isNull);
  });

  test('a Keep going with nothing kept carries the stop count and spend of '
      'the thread\'s last limit reply', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await api.replyAsAgent(
      aid,
      item.threadId,
      limitFailureReply(
        '10 turns',
        leftUsd: 3,
        progress: const LimitProgress(stops: 2, spentUsd: 3),
      ),
    );
    await loopWith([
      [for (var i = 0; i < 11; i++) const ScriptedCall('list_files', {})],
    ]).change(aid, [item], session: sid, keepGoing: true);

    expect(runner.states.single.ctx.changeSpentUsd, 3.0);
    expect(runner.states.single.budget.caps.costUsd, 1.5);
    final first = runner.text(0);
    expect(first, contains('has stopped at its limit twice'));
    expect(first, contains('too big for a change run'));
    expect(first, contains('no longer kept'));
    expect(
      bodies(item.threadId).last,
      contains('Stopped 3 times · \$3.00 of \$6 used'),
    );
    final kept = (await unfinished.get(item.threadId))!;
    expect(kept.stops, 3);
    expect(kept.spentUsd, 3.0);
  });

  test('a small change keeps the fast path: no plan, one run', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    final out = await loopWith([
      changeCalls(edits: [('Save', 'Keep')]),
    ]).change(aid, [item], session: sid);
    expect(out, isA<ChangeLanded>());
    expect(await plans.get(aid), isNull);
    expect(runner.seen, hasLength(1));
  });

  test('the change prompt asks the agent to plan a big change instead of '
      'building it', () async {
    final item = api.sendComment(aid, 'Add a stats screen', queue: false);
    await loopWith([changeCalls()]).change(aid, [item], session: sid);
    final first = runner.text(0);
    expect(first, contains('First decide whether this change fits one run'));
    expect(first, contains('plan_set'));
  });

  test(
    'the change prompt says plainly when the build has spent its \$6',
    () async {
      await plans.set(aid, const [
        PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
        PlanFeature(id: 'f2', title: 'Chart'),
      ], request: 'runs');
      await plans.spend(aid, buildCapUsd);
      final item = api.sendComment(aid, 'Add a stats screen', queue: false);
      await loopWith([changeCalls()]).change(aid, [item], session: sid);
      expect(runner.text(0), contains(r'This build has spent its $6'));
      expect(runner.text(0), contains('never that it is being built'));
    },
  );

  test('the unfinished file keeps a change however old it is, and past '
      'its size drops the oldest', () async {
    final dir = await _tempDir();
    final now = DateTime.utc(2026, 10, 15, 12);
    UnfinishedChange at(String t, DateTime when) => UnfinishedChange(
      artifactId: 'a',
      threadIds: [t],
      commentIds: const ['c'],
      limit: '10 turns',
      spentUsd: 1,
      at: when,
      notes: const ['n'],
      edits: {'index.html': 'x' * 400},
      before: const {'index.html': null},
    );
    final f = FileUnfinishedChanges(dir.file('u.json'), maxBytes: 1500);
    await f.put(at('month', now.subtract(const Duration(days: 30))));
    await f.put(at('day', now.subtract(const Duration(days: 2))));
    var again = FileUnfinishedChanges(dir.file('u.json'), maxBytes: 1500);
    expect((await again.get('month'))!.edits['index.html'], hasLength(400));
    expect((await again.get('day'))!.notes, ['n']);

    await again.put(at('fresh', now));
    again = FileUnfinishedChanges(dir.file('u.json'), maxBytes: 1500);
    expect(await again.get('month'), isNull);
    expect(await again.get('day'), isNotNull);
    expect(await again.get('fresh'), isNotNull);
    expect(dir.file('u.json').lengthSync(), lessThanOrEqualTo(1500));
  });

  test('the unfinished file reads nothing from a file past its size, and '
      'keeps no change that alone is past it', () async {
    final dir = await _tempDir();
    UnfinishedChange big(String t, int size) => UnfinishedChange(
      artifactId: 'a',
      threadIds: [t],
      commentIds: const ['c'],
      limit: '10 turns',
      spentUsd: 1,
      at: DateTime.utc(2026, 10, 15),
      edits: {'index.html': 'x' * size},
      before: const {'index.html': null},
    );
    // Written under a larger bound, read under a smaller one.
    await FileUnfinishedChanges(
      dir.file('u.json'),
      maxBytes: 5000,
    ).put(big('t1', 2000));
    expect(
      await FileUnfinishedChanges(dir.file('u.json'), maxBytes: 1500).get('t1'),
      isNull,
    );

    final f = FileUnfinishedChanges(dir.file('v.json'), maxBytes: 1500);
    await f.put(big('t2', 100));
    await f.put(big('t2', 3000));
    expect(await f.get('t2'), isNull);
    expect(
      await FileUnfinishedChanges(dir.file('v.json'), maxBytes: 1500).get('t2'),
      isNull,
    );
  });

  test('the unfinished file keeps a change across instances', () async {
    final dir = await _tempDir();
    final file = FileUnfinishedChanges(dir.file('unfinished.json'));
    final c = UnfinishedChange(
      artifactId: 'a',
      threadIds: const ['t1', 't2'],
      commentIds: const ['c1'],
      limit: r'$1.50',
      spentUsd: 1.5,
      at: DateTime.utc(2026, 10, 15),
      notes: const ['n'],
      answers: 'Units: "km"',
      lastRender: 'it showed an uncaught error',
      edits: const {'index.html': 'x', 'old.js': null},
      before: const {'index.html': 'y', 'old.js': 'z'},
      title: 'T',
    );
    await file.put(c);
    final again = FileUnfinishedChanges(dir.file('unfinished.json'));
    final read = (await again.get('t2'))!;
    expect(read.toJson(), c.toJson());
    await again.remove(['t1', 't2']);
    expect(
      await FileUnfinishedChanges(dir.file('unfinished.json')).get('t1'),
      isNull,
    );
  });

  group('limit words', () {
    test('name each cap in the person\'s words', () {
      expect(limitWords('10 turns'), 'more than 10 steps');
      expect(limitWords('120 seconds'), 'more than 2 minutes');
      expect(limitWords('60 seconds'), 'more than 1 minute');
      expect(limitWords('45 seconds'), 'more than 45 seconds');
      expect(limitWords('3 renders'), 'more than 3 checks of its work');
      expect(
        limitWords('32000 output tokens'),
        'more writing than one go '
        'allows',
      );
      expect(limitWords(r'$1.50'), r'more than its $1.50 a run');
    });

    test('Keep going says what is left once under half, and is not offered '
        'when nothing is', () {
      expect(keepGoingLine(5), keepGoingHint);
      expect(
        keepGoingLine(1.2),
        '$keepGoingHint About \$1.20 of its \$6 is '
        'left.',
      );
      expect(offersKeepGoing(keepGoingLine(0.01)), isFalse);
    });

    test('a Keep going reply is an app agent comment with the hint', () {
      ThreadComment c(String kind, String body, String? harness) =>
          ThreadComment.fromJson({
            'id': '1',
            'author_kind': kind,
            'author_name': 'Clax',
            'body': body,
            'created_at': '2026-10-15T00:00:00Z',
            'via_harness': ?harness,
          });
      expect(isKeepGoingReply(c('agent', keepGoingHint, 'app')), isTrue);
      expect(isKeepGoingReply(c('agent', keepGoingHint, 'claude')), isFalse);
      expect(isKeepGoingReply(c('viewer', keepGoingHint, null)), isFalse);
    });
  });
}

class _Dir {
  _Dir(this.path);
  final String path;
  File file(String name) => File('$path/$name');
}

Future<_Dir> _tempDir() async =>
    _Dir((await Directory.systemTemp.createTemp('unfinished')).path);
