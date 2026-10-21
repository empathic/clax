import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/agent/backend.dart' show AgentError;
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart' show Usage;

import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/ffi.dart';
import 'package:clax_app/queue/host.dart';
import 'package:clax_app/queue/isolate_client.dart' show builderStoppedError;
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/worker_agent.dart';
import 'package:clax_app/queue/worker_main.dart' show WorkerContext;
import 'package:clax_app/ui/render_service.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_inspector.dart';
import '../support/paths.dart';
import '../support/real_core.dart';
import 'queue_host_test.dart' show stateWhere;
import 'worker_agent_test.dart'
    show page, reachable, scriptedModel, script, system, unscriptedFile;

/// The queue end to end against the real embedded core: [QueueHost]
/// spawns the worker isolate, which attaches to the core the test opened
/// by its handle, runs the production agent ([workerAgent]) over a
/// scripted model, serves its drafts, records its renders at
/// `POST /api/drafts/{id}/renders` and commits its steps at the core's
/// job routes, render gate included.
void main() {
  test(
    'a make renders, finishes and publishes past the render gate, and ends '
    'done with its version',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      final (host, support) = startHost(core, scriptedModel);
      host.client.send(const Make('j1', 'a todo app', title: 'a todo app'));

      final job = await ended(host, support);
      expect(job.state, JobState.done, reason: '${job.error}');
      final aid = job.artifactId!;
      final detail = await api.getArtifact(aid);
      expect(detail.artifact.title, 'Todo');
      expect(detail.artifact.currentVersion, 1);
      expect(await api.readFiles(aid, 1), containsPair('index.html', page));
      expect(await api.readFiles(aid, 1), containsPair('app.js', script));
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );

  test(
    'a change of an app holding a binary file renders, publishes past the '
    'render gate, which counts the file it carries, and ends done',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      final session = await api.registerAppSession(
        installId: 'install-0',
        pid: pid,
        cwd: core.dir.path,
      );
      final transport = CoreTransport(core.host.calls);
      addTearDown(transport.close);
      final made = await transport.send(
        'POST',
        '/api/artifacts',
        headers: {
          'content-type': 'application/json',
          'x-clax-session': session.id,
        },
        body: Uint8List.fromList(
          utf8.encode(
            jsonEncode({
              'title': 'Todo',
              'files': {
                'index.html': {'content': page},
                'logo.png': {'content': base64Encode(logo), 'encoding': 'base64'},
              },
            }),
          ),
        ),
        timeout: const Duration(minutes: 1),
      );
      expect(made.status, 201, reason: utf8.decode(made.body));
      final aid =
          ((jsonDecode(utf8.decode(made.body)) as Map)['artifact'] as Map)['id']
              as String;

      final (host, support) = startHost(core, changeModel);
      host.client.send(
        Change('j1', aid, base: 1, threads: const {}, request: 'say hello'),
      );
      final job = await ended(host, support);
      expect(job.state, JobState.done, reason: '${job.error}');
      expect((await api.getArtifact(aid)).artifact.currentVersion, 2);
      expect(await api.readFiles(aid, 2), containsPair('index.html', changed));
      expect(await api.readFileBytes(aid, 2, 'logo.png'), logo);
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

/// A worker's queue on [core] with the production agent over [model]
/// and a fake inspector's clean renders, disposed in a tear-down; and
/// the worker's support directory.
(QueueHost, Directory) startHost(
  RealCore core,
  TurnBackend Function(WorkerContext context) model,
) {
  final support = Directory('${core.dir.path}/worker')..createSync();
  final render = RenderService(
    inspector: FakeInspector(),
    origins: RealCore.origins,
  );
  addTearDown(render.dispose);
  final host = QueueHost.start(
    coreHandle: core.host.handle,
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
      backend: model,
      probe: reachable,
    ),
    attach: attachEmbedded,
  );
  addTearDown(host.dispose);
  return (host, support);
}

/// [host]'s one job once it ends (or fails, or its worker stopped), after
/// checking the model was asked no turn it was not scripted for.
Future<JobView> ended(QueueHost host, Directory support) async {
  final job = (await stateWhere(
    host.client,
    (s) =>
        s.jobs.isNotEmpty &&
        (s.jobs.single.state.isFinal ||
            s.jobs.single.state == JobState.failed ||
            s.jobs.single.error == builderStoppedError),
  ).timeout(const Duration(minutes: 2))).jobs.single;
  final unscripted = File('${support.path}/$unscriptedFile');
  expect(
    unscripted.existsSync() ? unscripted.readAsStringSync() : null,
    isNull,
    reason: 'the model was asked a turn not scripted',
  );
  return job;
}

/// A PNG's first bytes: a file no text read holds.
final logo = Uint8List.fromList([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 1, 2]);

const changed = '<title>Todo</title><p>Hello</p>';

/// The model the worker asks for a change, built in the worker isolate.
TurnBackend changeModel(WorkerContext context) =>
    _ChangeModel(context.start.supportDir);

/// A change in two turns: the page rewritten and rendered, then finish.
/// Any other turn, or a turn after a result that is an error, is written
/// to [unscriptedFile] and answered with an [AgentError].
final class _ChangeModel implements TurnBackend {
  _ChangeModel(this.dir);
  final String dir;

  @override
  String get name => 'scripted';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    final asked = [
      for (final m in r.messages)
        if (m['role'] == 'assistant') m,
    ].length;
    final last = jsonEncode(r.messages.last['content']);
    if (asked > 1 || last.contains('"is_error":true')) {
      File('$dir/$unscriptedFile').writeAsStringSync(
        'turn after $asked asked: ${jsonEncode(r.messages)}',
      );
      throw AgentError('turn after $asked was not scripted');
    }
    return ModelTurn(
      content: asked == 0
          ? [
              _use('w', 'write_file', {'path': 'index.html', 'content': changed}),
              _use('r', 'render', {}),
            ]
          : [
              _use('f', 'finish', {'reply': 'Said hello.'}),
            ],
      stopReason: 'tool_use',
      usage: const Usage(input: 100, output: 50),
    );
  }

  static Map<String, Object?> _use(
    String id,
    String name,
    Map<String, Object?> input,
  ) => {'type': 'tool_use', 'id': id, 'name': name, 'input': input};
}

/// The worker's attachment to the core the test opened, over the library
/// `CLAX_EMBED_LIB` names.
CoreCalls attachEmbedded(int handle) =>
    CoreClient.attach(handle, lib: CoreLib(DynamicLibrary.open(claxEmbedLib()!)));
