// The live build eval's harness (tool/eval/) on a scripted agent: the
// real AgentLoop and FeedbackWorker over the fake daemon API and a fake
// inspector, so the harness is checked on every run of the suite without
// a model.
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../tool/eval/build_eval.dart';
import '../../tool/eval/eval_config.dart';
import '../agent/loop_support.dart' show until;
import '../agent/tools/support.dart' show StepClock;
import '../support/fake_inspector.dart';
import '../support/paths.dart';

final _promptFile = appPath('tool/eval/prompts.json');

Map<String, Object?> _f(String id, String title) => {
  'id': id,
  'title': title,
  'detail': '',
  'status': 'todo',
};

/// A first run: plans [features], builds the first, tests it, finishes.
List<ScriptedCall> _first(List<String> features) => [
  ScriptedCall('plan_set', {
    'features': [for (final f in features) _f(f, 'Feature $f')],
  }, 'Planning'),
  ScriptedCall('plan_mark', {'id': features.first, 'status': 'doing'}),
  const ScriptedCall('set_meta', {
    'title': 'Habits',
    'capabilities_json': '{"db": {}}',
  }),
  const ScriptedCall('write_file', {
    'path': 'index.html',
    'content': '<!doctype html><title>Habits</title><h1>Habits</h1>',
  }),
  const ScriptedCall('write_file', {
    'path': 'tests/app.test.js',
    'content': 'test("title", () => expect(1).toBe(1));',
  }),
  // The first render has a page error (its tests do not run), the
  // second failing tests (the fake's queued answers); after each a fix,
  // and the third passes.
  const ScriptedCall('render', {'file': null}),
  const ScriptedCall('str_replace', {
    'path': 'index.html',
    'old_str': '</h1>',
    'new_str': '</h1><p>Today</p>',
  }),
  const ScriptedCall('render', {'file': null}),
  const ScriptedCall('str_replace', {
    'path': 'index.html',
    'old_str': '</p>',
    'new_str': '</p><p>Streaks</p>',
  }),
  const ScriptedCall('render', {'file': null}),
  const ScriptedCall('write_file', {
    'path': 'icon.svg',
    'content':
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
        '<rect width="1024" height="1024" fill="#2a6"/></svg>',
  }),
  ScriptedCall('plan_mark', {'id': features.first, 'status': 'done'}),
  const ScriptedCall('finish', {'reply': 'Made it.', 'resolve': false}),
];

/// A continue run that builds [id].
List<ScriptedCall> _next(String id, String from, String to) => [
  ScriptedCall('plan_mark', {'id': id, 'status': 'doing'}),
  ScriptedCall('str_replace', {
    'path': 'index.html',
    'old_str': from,
    'new_str': to,
  }),
  const ScriptedCall('render', {'file': null}),
  ScriptedCall('plan_mark', {'id': id, 'status': 'done'}),
  ScriptedCall('finish', {'reply': 'Built $id.', 'resolve': true}),
];

class _Shots implements AppShots {
  _Shots(this.inspection);

  final Inspection Function({List<({String kind, String text})> errors})
  inspection;
  final apps = <String>[];

  @override
  Future<({Map<String, Inspection> shots, Map<String, String> errors})> capture(
    String aid,
  ) async {
    apps.add(aid);
    return (
      shots: {
        'phone': inspection(errors: const []),
        'desktop': inspection(
          errors: const [(kind: 'error', text: 'boom at 1280')],
        ),
      },
      errors: const <String, String>{},
    );
  }
}

class _Grants implements AppGrants {
  final seen = <String, Map<String, Object?>>{};

  @override
  Future<List<String>> grant(String aid, Map<String, Object?> caps) async {
    seen[aid] = caps;
    return ['net (api.example.com)'];
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('config', () {
    EvalConfig parse(List<String> args) => EvalConfig.parse(
      args,
      promptFile: _promptFile,
      outRoot: '/tmp/eval-out',
      now: DateTime(2026, 10, 13, 9, 5, 7),
    );

    test('the build cap the guard counts is the plan\'s', () {
      expect(perBuildCapUsd, buildCapUsd);
    });

    test('the spend guard refuses when the builds could spend more than '
        '--max-spend', () {
      expect(
        () => parse(['--max-spend', '20']),
        throwsA(
          isA<EvalConfigError>().having(
            (e) => e.message,
            'message',
            allOf(contains('refusing to start'), contains('\$30.00')),
          ),
        ),
      );
      final ok = parse(const []);
      expect(ok.prompts, hasLength(5));
      expect(ok.worstCaseUsd, 30);
      expect(ok.maxSpendUsd, 30);
    });

    test('a subset, a model and a run name', () {
      final c = parse([
        '--prompts',
        'reading,split',
        '--model',
        'claude-opus-5-5',
        '--max-spend',
        '12',
        '--run',
        'r1',
      ]);
      expect([for (final p in c.prompts) p.id], ['reading', 'split']);
      expect(c.model, 'claude-opus-5-5');
      expect(c.outDir, '/tmp/eval-out/r1');
      expect(parse(const ['--prompts', 'habits']).runName, '20261013-090507');
    });

    test('unknown models, prompts and arguments are refused', () {
      for (final args in [
        ['--model', 'gpt-9'],
        ['--prompts', 'nope'],
        ['--frobnicate'],
        ['--run', '../x'],
        ['--max-spend'],
      ]) {
        expect(
          () => parse(args),
          throwsA(isA<EvalConfigError>()),
          reason: '$args',
        );
      }
    });

    test('the prompt set covers the kit, db, charts, net, sample and a '
        'device capability', () {
      final covers = {for (final p in loadPrompts(_promptFile)) ...p.covers};
      expect(
        covers,
        containsAll(['kit', 'db', 'charts', 'net', 'sample', 'camera']),
      );
    });
  });

  group('builds', () {
    late FakeClaxApi api;
    late FeedbackWorker worker;
    late Directory out;

    tearDown(() async {
      await worker.stop();
      api.releasePolls();
      if (out.existsSync()) out.deleteSync(recursive: true);
    });

    test('each build runs to its end and writes its files, screenshots, '
        'plan, runs and the results table', () async {
      out = Directory.systemTemp.createTempSync('clax-eval-');
      api = FakeClaxApi();
      final clock = StepClock();
      final fake = FakeInspector()
        ..answer(
          FakeInspector().clean(
            errors: const [(kind: 'error', text: 'x is undefined')],
          ),
        )
        ..answerTests(
          const TestRun(
            passed: 2,
            failed: [(name: 'streak', error: 'expected 3')],
            skipped: 0,
          ),
        );
      final log = EvalLog(clock);
      final timings = EvalTimings(log);
      final eyes = RecordingInspector(fake, log);
      final runner = ScriptedRunner.runs([
        // habits: first run, one continue run, done.
        _first(['f1', 'f2']),
        _next('f2', 'Habits</h1>', 'Habits</h1><p>Best streak</p>'),
        // split: the model fails in the first run.
        const [ScriptedCall.fail(AgentError('the model is overloaded'))],
        // workouts: three features; the harness stops after one continue run.
        _first(['f1', 'f2', 'f3']),
        _next('f2', 'Habits</h1>', 'Habits</h1><p>Week</p>'),
        _next('f3', 'Week', 'Weekly totals'),
      ]);
      final plans = PlanStore();
      final loop = AgentLoop(
        api: api,
        runner: () => runner,
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: timings,
        inspector: eyes,
        baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
        plans: plans,
        clock: clock,
      );
      worker = FeedbackWorker(
        api: api,
        loop: loop,
        session: () async =>
            (await api.registerAppSession(installId: 'eval', pid: 1)).id,
        delay: (d) async {},
      )..start();
      await until(() => worker.sessionId != null, reason: 'registered');
      final shots = _Shots(fake.clean);
      final grants = _Grants();
      final eval = BuildEval(
        loop: loop,
        worker: worker,
        api: api,
        timings: timings,
        inspector: eyes,
        outDir: out.path,
        shots: shots,
        grants: grants,
        clock: clock,
        maxContinueRuns: 1,
      );
      final prompts = {for (final p in loadPrompts(_promptFile)) p.id: p};
      final config = EvalConfig.parse(
        const ['--prompts', 'habits,split,workouts', '--run', 'smoke'],
        promptFile: _promptFile,
        outRoot: out.path,
        now: clock.now(),
      );
      final builds = [
        for (final id in ['habits', 'split', 'workouts'])
          await eval.build(prompts[id]!),
      ];
      writeResults(out.path, config, builds);

      // How each build ended.
      expect(
        [for (final b in builds) b.ending],
        [Ending.done, Ending.error, Ending.cap],
        reason: [for (final b in builds) b.reason].join('; '),
      );
      expect(builds[1].reason, contains('the model is overloaded'));
      expect(builds[2].reason, 'harness: 1 continue runs');
      expect((await plans.get(builds[2].appId!))!.stopped, isTrue);

      // The finished build.
      final habits = builds[0];
      expect([for (final r in habits.runs) r.kind], ['first', 'continue']);
      expect([for (final r in habits.runs) r.version], [1, 2]);
      expect(habits.versions, 2);
      expect(habits.firstUsable, isNotNull);
      expect(habits.featuresDone, 2);
      expect(habits.featuresTotal, 2);
      expect(habits.turns, 13 + 5);
      expect(habits.renderErrors, 1);
      expect(habits.finalErrors, 1);
      expect(habits.granted, ['net (api.example.com)']);
      expect(grants.seen[habits.appId], {'db': <String, Object?>{}});
      final first = habits.runs.first;
      expect(first.outcome, 'finished');
      expect(
        [for (final t in first.tests) t.failed],
        [
          ['streak: expected 3'],
          isEmpty,
        ],
      );
      expect(habits.lastTests!.passed, 1);
      expect(first.steps, contains('plan_set'));
      expect(first.steps, contains('  "Planning"'));

      final dir = '${out.path}/habits';
      expect(
        File('$dir/files/index.html').readAsStringSync(),
        contains('Best streak'),
      );
      expect(File('$dir/files/tests/app.test.js').existsSync(), isTrue);
      expect(File('$dir/phone.png').readAsBytesSync(), fake.png);
      expect(File('$dir/desktop.png').existsSync(), isTrue);
      expect(Directory('$dir/renders').listSync(), hasLength(4));
      final plan = jsonDecode(File('$dir/plan.json').readAsStringSync()) as Map;
      expect(
        [for (final f in plan['features'] as List) f['status']],
        ['done', 'done'],
      );
      final runs =
          jsonDecode(File('$dir/runs.json').readAsStringSync()) as List;
      expect(runs, hasLength(2));
      expect((runs.first as Map)['inspections'], hasLength(3));
      final result =
          jsonDecode(File('$dir/result.json').readAsStringSync()) as Map;
      expect(result['ending'], 'done');
      expect(result['screenshots'], {
        'phone': 'phone.png',
        'desktop': 'desktop.png',
      });
      expect(File('$dir/steps.log').readAsStringSync(), contains('# run 1'));

      // The failed build made no app, and has no app files or screenshots.
      expect(Directory('${out.path}/split/files').existsSync(), isFalse);
      expect(
        jsonDecode(File('${out.path}/split/result.json').readAsStringSync()),
        containsPair('ending', 'error'),
      );
      expect(shots.apps, [habits.appId, builds[2].appId]);

      // The table: a row per build, and the rubric's columns.
      final md = File('${out.path}/RESULTS.md').readAsStringSync();
      expect(md, contains('| Works | Depth | Visual | Tests score | Notes |'));
      expect(md, contains('| [habits](habits/) | done | 2 | 2/2 | 2 |'));
      expect(md, contains('| [split](split/) | error | 1 | 0/0 | 0 |'));
      expect(md, contains('| [workouts](workouts/) | cap | 2 | 2/3 | 2 |'));
      expect(md, contains('## Rubric'));
      expect(md, contains('boom at 1280'));
    });
  });
}
