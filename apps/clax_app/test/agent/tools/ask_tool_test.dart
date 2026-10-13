import 'dart:async';

import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/ask_tool.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support.dart';

/// A timer the test fires at will.
class ManualTimer implements Timer {
  ManualTimer(this.duration, this._fire);

  final Duration duration;
  final void Function() _fire;
  bool _active = true;

  void fire() {
    if (!_active) return;
    _active = false;
    _fire();
  }

  @override
  void cancel() => _active = false;

  @override
  bool get isActive => _active;

  @override
  int get tick => 0;
}

Map<String, Object?> okJson(ToolOutput r) {
  expect(r, isA<ToolOk>(), reason: '$r');
  return (r as ToolOk).json;
}

Matcher refused(Object message) =>
    isA<ToolRefused>().having((e) => e.message, 'message', message);

const layout = {
  'question': 'Which layout should the list use?',
  'header': 'Layout',
  'options': [
    {
      'label': 'Cards',
      'description': 'One card per run',
      'preview': null,
      'recommended': true,
    },
    {
      'label': 'Table',
      'description': null,
      'preview': null,
      'recommended': null,
    },
  ],
  'multi_select': null,
  'other': null,
};

Map<String, Object?> asking({
  List<Object?>? questions = const [layout],
  String? questionId,
  int? timeoutS,
  bool? cancel,
}) => {
  'questions': questions,
  'question_id': questionId,
  'timeout_s': timeoutS,
  'cancel': cancel,
};

/// A fake daemon with an app session, this run's app and an open owner
/// surface (a stream holding `questions`).
class World {
  World() {
    api = FakeClaxApi(
      timer: (d, f) {
        final t = ManualTimer(d, f);
        timers.add(t);
        return t;
      },
    );
    app = api.seedArtifact('Runs', {'index.html': '<p>runs</p>'});
  }

  late final FakeClaxApi api;
  late final String app;
  late final String sid;
  final timers = <ManualTimer>[];
  final asked = MemoryAskedQuestions();
  final shown = <QuestionView>[];
  final clock = StepClock();
  final cancel = Completer<void>();
  late Toolbox toolbox;
  late AskTool tool;

  Future<void> start({bool newApp = false, bool surface = true}) async {
    sid = (await api.registerAppSession(installId: 'i', pid: 1)).id;
    if (surface) await api.stream().subscribe(['questions']);
    final kind = newApp ? RequestKind.newApp : RequestKind.change;
    final items = newApp
        ? const <FeedbackItem>[]
        : [
            FeedbackItem(
              feedbackId: 'f1',
              threadId: 't1',
              commentId: 'c1',
              artifactId: app,
              body: 'Make it nicer',
              author: 'Alex',
              version: 1,
            ),
            FeedbackItem(
              feedbackId: 'f2',
              threadId: 't2',
              commentId: 'c2',
              artifactId: app,
              body: 'And sort it',
              author: 'Alex',
              version: 1,
            ),
          ];
    final ctx = RunContext(
      kind: kind,
      artifactId: newApp ? null : app,
      baseVersion: newApp ? 0 : 1,
      title: 'Runs',
      files: const {'index.html': '<p>runs</p>'},
      items: items,
      threadIds: [for (final i in items) i.threadId],
      sessionId: sid,
      cancelled: cancel.future,
      clock: clock,
    );
    tool = AskTool(
      api,
      asked,
      maxWaitNew: askWaitNewApp,
      maxWaitChange: askWaitChange,
      onAsked: shown.add,
    );
    toolbox = Toolbox(
      RunState(
        ctx: ctx,
        draft: Draft(
          files: ctx.files,
          title: 'Runs',
          baseVersion: ctx.baseVersion,
        ),
        budget: RunBudget(
          newApp ? testNewCaps : testChangeCaps,
          'claude-haiku-5-5',
          clock,
        ),
      ),
      [tool],
    );
  }

  RunBudget get budget => toolbox.state.budget;

  /// Asks and lets the call reach its wait.
  Future<Future<ToolOutput>> ask(Map<String, Object?> input) async {
    final out = toolbox.call('ask', input);
    await pumpEventQueue();
    return out;
  }

  String get qid => shown.last.id;

  List<FakeCall> callsOf(String name) => [
    for (final c in api.calls)
      if (c.name == name) c,
  ];
}

void main() {
  late World w;

  setUp(() => w = World());

  test('the spec is not strict, with Clax ask arguments', () {
    final spec = AskTool(
      w.api,
      w.asked,
      maxWaitNew: askWaitNewApp,
      maxWaitChange: askWaitChange,
    ).spec;
    expect(spec.name, 'ask');
    expect(spec.strict, isFalse);
    expect(spec.toMessagesApi().containsKey('strict'), isFalse);
    expect(spec.eager, isFalse);
    final schema = spec.inputSchema;
    expect((schema['properties']! as Map).keys, [
      'questions',
      'question_id',
      'timeout_s',
      'cancel',
    ]);
    void strictObjects(Object? s) {
      if (s is Map) {
        if (s['type'] == 'object') {
          expect(s['additionalProperties'], false);
          expect(
            (s['properties']! as Map).keys.toSet(),
            containsAll(s['required']! as List),
          );
        }
        s.values.forEach(strictObjects);
      } else if (s is List) {
        s.forEach(strictObjects);
      }
    }

    strictObjects(schema);
  });

  group('a change', () {
    setUp(() => w.start());

    test('answered within the wait', () async {
      final out = await w.ask(asking());
      expect(w.shown, hasLength(1));
      expect(w.budget.elapsed, Duration.zero);
      await w.api.answerQuestion(w.qid, [
        {
          'selected': ['Cards'],
          'text': null,
        },
      ]);
      final j = okJson(await out);
      expect(j, {
        'question_id': w.qid,
        'status': 'answered',
        'reply': [
          {
            'question': 'Which layout should the list use?',
            'header': 'Layout',
            'selected': ['Cards'],
            'text': null,
          },
        ],
        'call_again': false,
        'note': askNote,
      });
      expect(await w.asked.all(), isEmpty);
      expect(w.api.questionOf(w.qid).status, 'answered');
    });

    test('asks about the app, with nulls left out of the question', () async {
      await w.ask(asking());
      final call = w.callsOf('askQuestion').single;
      expect(call.args['sid'], w.sid);
      expect(call.args['artifactId'], w.app);
      expect(call.args['questions'], [
        {
          'question': 'Which layout should the list use?',
          'header': 'Layout',
          'options': [
            {
              'label': 'Cards',
              'description': 'One card per run',
              'recommended': true,
            },
            {'label': 'Table'},
          ],
        },
      ]);
    });

    test('declined', () async {
      final out = await w.ask(asking());
      await w.api.declineQuestion(w.qid);
      final j = okJson(await out);
      expect(j['status'], 'declined');
      expect(j['reply'], isNull);
      expect(j['call_again'], false);
      expect(j.containsKey('surface_open'), isFalse);
      expect(await w.asked.all(), isEmpty);
    });

    test('still open when the wait ends: call_again, and the question is '
        'remembered with the run\'s threads and comments', () async {
      final out = await w.ask(asking());
      final remembered = (await w.asked.all()).single;
      expect(remembered.questionId, w.qid);
      expect(remembered.artifactId, w.app);
      expect(remembered.threadIds, ['t1', 't2']);
      expect(remembered.commentIds, ['c1', 'c2']);
      w.timers.single.fire();
      final j = okJson(await out);
      expect(j, {
        'question_id': w.qid,
        'status': 'open',
        'reply': null,
        'call_again': true,
        'surface_open': true,
        'note': askNote,
      });
      expect((await w.asked.all()).single.questionId, w.qid);
      expect(w.api.questionOf(w.qid).status, 'open');
    });

    test('question_id waits again and takes the answer', () async {
      final first = await w.ask(asking());
      w.timers.single.fire();
      expect(okJson(await first)['status'], 'open');
      final again = await w.ask(asking(questions: null, questionId: w.qid));
      expect(w.callsOf('askQuestion'), hasLength(1));
      expect(w.timers, hasLength(2));
      await w.api.answerQuestion(w.qid, [
        {'selected': <String>[], 'text': 'Neither: a calendar'},
      ]);
      final j = okJson(await again);
      expect(j['status'], 'answered');
      expect(
        (j['reply']! as List).single,
        containsPair('text', 'Neither: a calendar'),
      );
      expect(await w.asked.all(), isEmpty);
    });

    test('cancel withdraws the question', () async {
      final first = await w.ask(asking());
      w.timers.single.fire();
      await first;
      final j = okJson(
        await w.toolbox.call(
          'ask',
          asking(questions: null, questionId: w.qid, cancel: true),
        ),
      );
      expect(j['status'], 'withdrawn');
      expect(j['call_again'], false);
      expect(w.api.questionOf(w.qid).status, 'withdrawn');
      expect(await w.asked.all(), isEmpty);
    });

    test(
      'cancel of a question answered meanwhile hands the answer over',
      () async {
        final first = await w.ask(asking());
        w.timers.single.fire();
        await first;
        await w.api.answerQuestion(w.qid, [
          {
            'selected': ['Table'],
            'text': null,
          },
        ]);
        final j = okJson(
          await w.toolbox.call(
            'ask',
            asking(questions: null, questionId: w.qid, cancel: true),
          ),
        );
        expect(j['status'], 'answered');
        expect(
          (j['reply']! as List).single,
          containsPair('selected', ['Table']),
        );
        // Taken by this call: the next feedback poll hands over only the
        // answer to another question.
        final poll = w.api.pollFeedback(w.sid);
        final other = await w.api.askQuestion(w.sid, [
          {'question': 'Other?', 'header': 'Other'},
        ]);
        await w.api.declineQuestion(other.question.id);
        expect(
          [for (final a in (await poll).answers) a.id],
          [other.question.id],
        );
      },
    );

    test('timeout_s 600 is waited as 45 s, and the run\'s wall clock stops '
        'while it waits', () async {
      final out = await w.ask(asking(timeoutS: 600));
      expect(w.timers.single.duration, const Duration(seconds: 45));
      expect(
        w.callsOf('waitQuestion').single.args['wait'],
        const Duration(seconds: 45),
      );
      w.clock.advance(const Duration(seconds: 45));
      expect(w.budget.elapsed, Duration.zero);
      expect(w.budget.timeUp, isFalse);
      w.timers.single.fire();
      await out;
      w.clock.advance(const Duration(seconds: 2));
      expect(w.budget.elapsed, const Duration(seconds: 2));
    });

    test(
      'timeout_s below the cap is waited as given, and raised to 1',
      () async {
        final a = await w.ask(asking(timeoutS: 10));
        expect(w.timers.last.duration, const Duration(seconds: 10));
        w.timers.last.fire();
        await a;
        final b = await w.ask(
          asking(questions: null, questionId: w.qid, timeoutS: 0),
        );
        expect(w.timers.last.duration, const Duration(seconds: 1));
        w.timers.last.fire();
        await b;
        final c = await w.ask(asking(questions: null, questionId: w.qid));
        expect(w.timers.last.duration, const Duration(seconds: 45));
        w.timers.last.fire();
        await c;
      },
    );

    test('a second set of questions is refused', () async {
      final first = await w.ask(asking());
      w.timers.single.fire();
      await first;
      expect(
        await w.toolbox.call('ask', asking()),
        refused(allOf(contains('already asked'), contains(w.qid))),
      );
      expect(w.callsOf('askQuestion'), hasLength(1));
    });

    test('invalid_question comes back with the daemon\'s message, and the '
        'run may ask again', () async {
      final bad = {...layout, 'header': 'A header far too long'};
      final out = await w.toolbox.call('ask', asking(questions: [bad]));
      expect(out, refused(contains('header')));
      expect(w.shown, isEmpty);
      expect(await w.asked.all(), isEmpty);
      final ok = await w.ask(asking());
      w.timers.single.fire();
      expect(okJson(await ok)['status'], 'open');
    });

    test('limit_reached comes back with the daemon\'s message', () async {
      w.api.failNext(
        'askQuestion',
        const ClaxApiError(
          status: 429,
          code: 'limit_reached',
          message: 'at most 8 open questions per session and 100 in all',
        ),
      );
      expect(
        await w.toolbox.call('ask', asking()),
        refused(contains('at most 8 open questions')),
      );
    });

    test('arguments are checked as Clax ask checks them', () async {
      expect(
        await w.toolbox.call('ask', asking(questions: null)),
        refused(contains('pass questions to ask')),
      );
      expect(
        await w.toolbox.call('ask', asking(questionId: 'q')),
        refused(contains('pass questions to ask')),
      );
      expect(
        await w.toolbox.call('ask', asking(cancel: true)),
        refused(contains('cancel goes with question_id')),
      );
      expect(
        await w.toolbox.call('ask', {...asking(), 'url_or_id': 'x'}),
        refused(contains('url_or_id')),
      );
      expect(
        await w.toolbox.call('ask', asking(questions: ['not a question'])),
        refused(contains('questions')),
      );
      expect(w.callsOf('askQuestion'), isEmpty);
    });

    test('question_id must name the question this run asked', () async {
      final other = await w.api.askQuestion(w.sid, [
        {'question': 'Other run?', 'header': 'Other'},
      ]);
      expect(
        await w.toolbox.call(
          'ask',
          asking(questions: null, questionId: other.question.id),
        ),
        refused(contains('not a question this run asked')),
      );
      expect(w.callsOf('waitQuestion'), isEmpty);
    });

    test('a cancel of the run while waiting withdraws the question', () async {
      final out = await w.ask(asking());
      w.cancel.complete();
      expect(await out, refused(contains('cancelled')));
      expect(w.api.questionOf(w.qid).status, 'withdrawn');
      expect(await w.asked.all(), isEmpty);
      w.clock.advance(const Duration(seconds: 3));
      expect(w.budget.elapsed, const Duration(seconds: 3));
    });

    test(
      'a cancel of the run after the wait withdraws the open question',
      () async {
        final out = await w.ask(asking());
        w.timers.single.fire();
        await out;
        w.cancel.complete();
        await pumpEventQueue();
        expect(w.api.questionOf(w.qid).status, 'withdrawn');
        expect(await w.asked.all(), isEmpty);
      },
    );

    test('a call after the run was cancelled asks nothing', () async {
      w.cancel.complete();
      expect(
        await w.toolbox.call('ask', asking()),
        refused(contains('cancelled')),
      );
      expect(w.callsOf('askQuestion'), isEmpty);
    });

    test(
      'a question gone from the daemon (404 not_found) is forgotten',
      () async {
        final first = await w.ask(asking());
        w.timers.single.fire();
        await first;
        w.api.failNext(
          'waitQuestion',
          const ClaxApiError(
            status: 404,
            code: 'not_found',
            message: 'no such question',
          ),
        );
        expect(
          await w.toolbox.call(
            'ask',
            asking(questions: null, questionId: w.qid),
          ),
          refused(contains('gone')),
        );
        expect(await w.asked.all(), isEmpty);
      },
    );

    test(
      'a wait cut off by the daemon keeps the question open and names it',
      () async {
        w.api.failNext(
          'waitQuestion',
          const ClaxApiError(
            status: 0,
            code: 'timeout',
            message: 'no answer in time',
          ),
        );
        final out = await w.toolbox.call('ask', asking());
        expect(out, refused(allOf(contains(w.qid), contains('question_id'))));
        expect((await w.asked.all()).single.questionId, w.qid);
        expect(w.budget.elapsed, Duration.zero);
        w.clock.advance(const Duration(seconds: 1));
        expect(w.budget.elapsed, const Duration(seconds: 1));
      },
    );

    test('withdrawOpen withdraws an open question once', () async {
      final out = await w.ask(asking());
      w.timers.single.fire();
      await out;
      expect(w.tool.questionId, w.qid);
      await Future.wait([w.tool.withdrawOpen(), w.tool.withdrawOpen()]);
      expect(w.api.questionOf(w.qid).status, 'withdrawn');
      expect(w.callsOf('withdrawQuestion'), hasLength(1));
      expect(await w.asked.all(), isEmpty);
    });
  });

  group('no owner surface open', () {
    setUp(() => w.start(surface: false));

    test('surface_open false comes back at once, without a wait', () async {
      final j = okJson(await w.toolbox.call('ask', asking()));
      expect(j['status'], 'open');
      expect(j['call_again'], true);
      expect(j['surface_open'], false);
      expect(w.callsOf('waitQuestion'), isEmpty);
      expect((await w.asked.all()).single.questionId, w.qid);
    });

    test('an explicit question_id waits, and keeps surface_open', () async {
      okJson(await w.toolbox.call('ask', asking()));
      final again = await w.ask(asking(questions: null, questionId: w.qid));
      w.timers.single.fire();
      expect(okJson(await again)['surface_open'], false);
    });
  });

  group('a new app', () {
    setUp(() => w.start(newApp: true));

    test(
      'asks about no artifact, waits up to 300 s, remembers nothing',
      () async {
        final out = await w.ask(asking(timeoutS: 600));
        expect(w.callsOf('askQuestion').single.args['artifactId'], isNull);
        expect(w.timers.single.duration, const Duration(seconds: 300));
        expect(w.shown.single.status, 'open');
        expect(await w.asked.all(), isEmpty);
        w.clock.advance(const Duration(seconds: 300));
        w.timers.single.fire();
        expect(okJson(await out)['status'], 'open');
        expect(w.budget.elapsed, Duration.zero);
      },
    );

    test('Cancel withdraws the question it waits on', () async {
      final out = await w.ask(asking());
      w.cancel.complete();
      expect(await out, refused(contains('cancelled')));
      expect(w.api.questionOf(w.qid).status, 'withdrawn');
    });
  });

  test('a run without a session cannot ask', () async {
    await w.start();
    final ctx = w.toolbox.state.ctx;
    final box = Toolbox(
      RunState(
        ctx: RunContext(
          kind: ctx.kind,
          artifactId: ctx.artifactId,
          files: ctx.files,
          clock: w.clock,
        ),
        draft: Draft(files: ctx.files, baseVersion: 1),
        budget: RunBudget(testChangeCaps, 'claude-haiku-5-5', w.clock),
      ),
      [
        AskTool(
          w.api,
          w.asked,
          maxWaitNew: askWaitNewApp,
          maxWaitChange: askWaitChange,
        ),
      ],
    );
    expect(await box.call('ask', asking()), refused(contains('session')));
    expect(w.callsOf('askQuestion'), isEmpty);
  });
}
