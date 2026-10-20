import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart' show AgentError;
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart' show Usage;
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/queue/agent_port.dart' show draftDigest;
import 'package:clax_app/queue/host.dart';
import 'package:clax_app/queue/isolate_client.dart' show builderStopped;
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/worker_agent.dart';
import 'package:clax_app/queue/worker_main.dart' show WorkerContext;
import 'package:clax_app/ui/render_service.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_core.dart';
import '../support/fake_core_jobs.dart';
import '../support/fake_inspector.dart';
import '../support/port_core.dart';
import 'queue_host_test.dart' show stateWhere;

/// A make end to end on the production agent ([workerAgent]): [QueueHost]
/// spawns a real worker isolate whose agent is lib/agent's tools and
/// prompts over the worker's own attachment to [FakeCore] (its job routes
/// [FakeJobs], sessions, drafts and renders), asking a scripted model
/// ([_ScriptedModel]) that fails loudly on any turn it was not written
/// for, and rendering in the UI isolate's [RenderService] over a fake
/// inspector.
void main() {
  const origins = AppOrigins('clax://app', 'clax://{id}.c');
  late Directory support;
  QueueHost? host;
  final closers = <void Function()>[];

  setUp(() {
    support = Directory.systemTemp.createTempSync('clax-worker-agent-');
  });

  tearDown(() async {
    await host?.dispose();
    host = null;
    for (final c in closers) {
      c();
    }
    closers.clear();
    final unscripted = File('${support.path}/$unscriptedFile');
    final said = unscripted.existsSync() ? unscripted.readAsStringSync() : null;
    support.deleteSync(recursive: true);
    expect(said, isNull, reason: 'the model was asked a turn not scripted');
  });

  /// A core with the job routes and what the worker's agent calls besides:
  /// its session, the person's apps, drafts and their publishes.
  ({FakeCore core, FakeJobs jobs, PortCoreServer server, List<Object?> drafts})
  coreWith() {
    final core = FakeCore();
    final jobs = FakeJobs(core);
    final drafts = <Object?>[];
    core
      ..on('POST', '/api/sessions', (r, _) {
        final b = FakeCore.bodyOf(r)! as Map;
        return FakeCore.json({
          'session': {
            'id': 's1',
            'harness': b['harness'],
            'harness_session_id': b['harness_session_id'],
            'cwd': b['cwd'],
            'pid': b['pid'],
          },
        });
      })
      ..on('GET', '/api/artifacts', (r, _) => FakeCore.json({'artifacts': []}))
      ..on('POST', '/api/drafts', (r, _) {
        drafts.add(FakeCore.bodyOf(r));
        return FakeCore.json({
          'draft': {'id': 'd${drafts.length.toString().padLeft(11, '0')}', 'version': 1},
        }, status: 201);
      });
    final server = PortCoreServer(core, jobs: jobs);
    closers.add(server.close);
    return (core: core, jobs: jobs, server: server, drafts: drafts);
  }

  QueueHost start(PortCoreServer server, FakeInspector inspector) {
    final render = RenderService(inspector: inspector, origins: origins);
    closers.add(render.dispose);
    return host = QueueHost.start(
      coreHandle: 7,
      supportDir: support.path,
      settings: const {'model': 'claude-haiku-5-5'},
      apiKey: 'key',
      render: render,
      agent: workerAgent(
        const WorkerAgentAssets(
          system: system,
          kit: {},
          installId: 'install-1',
          pid: 1,
        ),
        backend: scriptedModel,
        probe: reachable,
      ),
      attach: server.attach,
    );
  }

  Future<JobView> ended(QueueHost h) async => (await stateWhere(
    h.client,
    (s) =>
        s.jobs.isNotEmpty &&
        (s.jobs.single.state.isFinal ||
            s.jobs.single.state == JobState.failed ||
            s.jobs.single.error == builderStopped),
  )).jobs.single;

  test('a make ends published, with its render recorded clean; the worker '
      'dying mid-make resumes it with every earlier turn\'s writes', () async {
    File('${support.path}/$dieFile').writeAsStringSync('once');
    final c = coreWith();
    final inspector = FakeInspector();
    final h = start(c.server, inspector);
    h.client.send(const Make('j1', 'a todo app', title: 'a todo app'));

    final job = await ended(h);
    expect(job.state, JobState.done, reason: '${job.error}');
    expect(
      File('${support.path}/$diedFile').existsSync(),
      isTrue,
      reason: 'the worker died at turn 3',
    );
    // The draft rendered held turn 1's page and turn 2's script: the
    // worker that resumed ran turn 1's calls again from the rows.
    const files = {'index.html': page, 'app.js': script};
    expect((c.drafts.single! as Map)['files'], files);
    expect(inspector.calls, hasLength(1));
    expect(c.jobs.renders, [
      {
        'draft': 'd00000000001',
        'digest': draftDigest(files),
        'errors': <Object?>[],
        'failed_tests': <Object?>[],
      },
    ]);
    final published = c.jobs.published.single;
    expect(published['draft'], {
      'id': 'd00000000001',
      'digest': draftDigest(files),
    });
    expect(published['request'], {
      'title': 'Todo',
      'note': 'Made a todo app.',
      'files': {
        'index.html': {'content': page},
        'app.js': {'content': script},
        'icon.svg': {'content': icon},
      },
    });
    expect(published['session_id'], 's1');
    expect(c.jobs.row('j1')['state'], 'done');
    expect(
      [for (final s in c.jobs.jobs['j1']!['steps']! as List) (s as Map)['key']],
      ['turn:1', 'turn:2', 'turn:3', 'turn:4', 'publish:0'],
    );
  });

  test('a host started fresh over the same rows does what another fresh '
      'host does, from the rows alone', () async {
    Future<Map<String, Object?>> runOnce() async {
      final c = coreWith();
      _seedMidMake(c.jobs);
      final inspector = FakeInspector();
      final h = start(c.server, inspector);
      final job = await ended(h);
      await h.dispose();
      host = null;
      expect(job.state, JobState.done, reason: '${job.error}');
      return {
        'drafts': c.drafts,
        'renders': c.jobs.renders,
        'published': c.jobs.published,
        'steps': [
          for (final s in c.jobs.jobs['j1']!['steps']! as List)
            (s as Map)['key'],
        ],
      };
    }

    final a = await runOnce();
    final b = await runOnce();
    expect(jsonEncode(b), jsonEncode(a));
    // What the rows held reached the publish: turn 1's page and title.
    final request = ((a['published']! as List).single as Map)['request'] as Map;
    expect((request['files'] as Map).keys, containsAll(['index.html', 'app.js']));
    expect(request['title'], 'Todo');
  });
}

const system = 'You make apps.';
const page = '<title>Todo</title><ul id="list"></ul><script src="app.js"></script>';
const icon =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
    '<rect width="1024" height="1024" fill="#0F766E"/></svg>';
const script = 'document.getElementById("list").textContent = "none yet";';

/// In the support directory: present, the worker dies once at turn 3;
/// written once it has; written with the request when a turn was not
/// scripted.
const dieFile = 'die-at-turn-3';
const diedFile = 'died';
const unscriptedFile = 'unscripted';

Future<bool> reachable() async => true;

/// The model the worker asks, built in the worker isolate.
TurnBackend scriptedModel(WorkerContext context) =>
    _ScriptedModel(context.start.supportDir);

/// A make in four turns: the page and its title, then its script, then a
/// render, then finish. Each turn is answered only when the request is
/// the one it was written for (the system prompt, the run's history so
/// far, every result clean); any other request is written to
/// [unscriptedFile] and answered with an [AgentError], which fails the
/// job.
final class _ScriptedModel implements TurnBackend {
  _ScriptedModel(this.dir);
  final String dir;

  @override
  String get name => 'scripted';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    final asked = [
      for (final m in r.messages)
        if (m['role'] == 'assistant') m,
    ].length;
    final tools = {for (final t in r.tools) t['name']};
    final results = switch (r.messages.last['content']) {
      final List c => [
        for (final b in c)
          if (b case final Map m when m['type'] == 'tool_result') m,
      ],
      _ => const <Map>[],
    };
    final clean = results.every((b) => b['is_error'] != true);
    final first = jsonEncode(r.messages.first['content']);
    final fits =
        r.system == system &&
        tools.containsAll(['write_file', 'set_meta', 'render', 'finish']) &&
        first.contains('a todo app') &&
        clean &&
        results.length == const [0, 2, 1, 1][asked.clamp(0, 3)];
    if (!fits || asked > 3) return _unscripted(r, asked);
    switch (asked) {
      case 0:
        return _answer([
          _use('w', 'write_file', {'path': 'index.html', 'content': page}),
          _use('m', 'set_meta', {'title': 'Todo'}),
        ]);
      case 1:
        return _answer([
          _use('s', 'write_file', {'path': 'app.js', 'content': script}),
        ]);
      case 2:
        final die = File('$dir/$dieFile');
        if (die.existsSync()) {
          die.deleteSync();
          File('$dir/$diedFile').writeAsStringSync('turn 3');
          // An Error ends the worker isolate; its client starts another.
          throw StateError('the worker dies here, once');
        }
        return _answer([_use('r', 'render', {})]);
      default:
        return _answer([
          _use('i', 'write_file', {'path': 'icon.svg', 'content': icon}),
          _use('f', 'finish', {'reply': 'Made a todo app.'}),
        ]);
    }
  }

  Never _unscripted(TurnRequest r, int asked) {
    File('$dir/$unscriptedFile').writeAsStringSync(
      'turn after $asked asked: ${jsonEncode(r.messages)}',
    );
    throw AgentError('turn after $asked was not scripted');
  }

  static ModelTurn _answer(List<Map<String, Object?>> content) => ModelTurn(
    content: content,
    stopReason: 'tool_use',
    usage: const Usage(input: 100, output: 50),
  );
}

Map<String, Object?> _use(String id, String name, Map<String, Object?> input) =>
    {'type': 'tool_use', 'id': id, 'name': name, 'input': input};

/// Job `j1` as a worker that died after its second turn left it in the
/// core: running, with `turn:1` (the page and its title) and `turn:2`
/// (its script) committed.
void _seedMidMake(FakeJobs jobs) {
  Map<String, Object?> turn(int n, List<Map<String, Object?>> uses) => {
    'key': 'turn:$n',
    'result': {
      'base': 0,
      'notes': {
        'calls': [
          for (final u in uses)
            {'id': u['id'], 'tool': u['name'], 'args': u['input']},
        ],
        'data': {
          'assistant': uses,
          'usage': {'input': 100, 'cache_write': 0, 'cache_read': 0, 'output': 50},
        },
      },
    },
    'version_n': null,
    'created_at': '2026-10-20T00:00:0${n}Z',
  };
  jobs.seed(
    'j1',
    state: 'running',
    step: 'turn:2',
    rev: 4,
    title: 'a todo app',
    input: const {'request': 'a todo app', 'base': 0, 'threads': {}},
    steps: [
      turn(1, [
        _use('w', 'write_file', {'path': 'index.html', 'content': page}),
        _use('m', 'set_meta', {'title': 'Todo'}),
      ]),
      turn(2, [
        _use('s', 'write_file', {'path': 'app.js', 'content': script}),
      ]),
    ],
  );
}
