import 'dart:ffi';
import 'dart:io';

import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/ffi.dart';
import 'package:clax_app/queue/host.dart';
import 'package:clax_app/queue/isolate_client.dart' show builderStoppedError;
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/worker_agent.dart';
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
          backend: scriptedModel,
          probe: reachable,
        ),
        attach: attachEmbedded,
      );
      addTearDown(host.dispose);
      host.client.send(const Make('j1', 'a todo app', title: 'a todo app'));

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
}

/// The worker's attachment to the core the test opened, over the library
/// `CLAX_EMBED_LIB` names.
CoreCalls attachEmbedded(int handle) =>
    CoreClient.attach(handle, lib: CoreLib(DynamicLibrary.open(claxEmbedLib()!)));
