import 'dart:async';

import 'package:clax_app/core/origins.dart';
import 'package:clax_app/queue/client.dart' show QueueClient;
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/host.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/runner.dart';
import 'package:clax_app/queue/worker_main.dart';
import 'package:clax_app/ui/inspector_surface.dart' show InspectorLost;
import 'package:clax_app/ui/render_service.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_core.dart';
import '../support/fake_core_jobs.dart';
import '../support/fake_inspector.dart';
import '../support/port_core.dart';

/// The queue joined end to end (spec 2026-10-19-in-process-core-design
/// §7): [QueueHost] spawns a real worker isolate running `update` and the
/// real [EffectRunner] over [FakeCore]'s job routes ([FakeJobs], served to
/// the worker through a port), a scripted agent in the worker, and the
/// UI isolate's [RenderService] over a fake inspector answering the
/// worker's renders across the isolate wire.
void main() {
  const origins = AppOrigins('clax://app', 'clax://{id}.c');
  late FakeCore core;
  late FakeJobs jobs;
  late PortCoreServer server;
  late FakeInspector inspector;
  late RenderService render;
  late List<Effect> said;
  QueueHost? host;

  setUp(() {
    core = FakeCore();
    jobs = FakeJobs(core);
    server = PortCoreServer(core, jobs: jobs);
    inspector = FakeInspector();
    render = RenderService(inspector: inspector, origins: origins);
    said = [];
  });

  tearDown(() async {
    await host?.dispose();
    host = null;
    render.dispose();
    server.close();
  });

  QueueHost start() => host = QueueHost.start(
    coreHandle: 7,
    supportDir: '/support',
    settings: const {'model': 'test'},
    apiKey: 'key',
    render: render,
    agent: scriptedAgent,
    onEffect: said.add,
    attach: server.attach,
  );

  test('a make goes to a published job, its render lost once on the way '
      'and asked again', () async {
    // The first render's WebView is torn down under it.
    inspector.fail(const InspectorLost('the engine detached'));
    final h = start();
    final seen = <JobState>[];
    h.client.state.addListener(() {
      final jobs = h.client.state.value.jobs;
      if (jobs.isNotEmpty && (seen.isEmpty || seen.last != jobs.single.state)) {
        seen.add(jobs.single.state);
      }
    });
    h.client.send(const Make('j1', 'a todo app', title: 'Todo'));

    final done = await stateWhere(
      h.client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state.isFinal,
    );
    final job = done.jobs.single;
    expect(job.state, JobState.done);
    expect(job.artifactId, isNotNull);
    expect(seen.first, JobState.queued);
    expect(seen.last, JobState.done);

    // Two renders of the one call: the lost one, and the last try.
    expect(inspector.calls, hasLength(2));
    expect(
      inspector.calls.first.url,
      Uri.parse('clax://app/a/$draftId?app=1&inspect=1'),
    );
    // The answered one was recorded under the draft's digest, clean, and
    // the publish step named that render, which the core's gate checked.
    expect(jobs.renders, [
      {
        'draft': draftId,
        'digest': digest,
        'errors': <Object?>[],
        'failed_tests': <Object?>[],
      },
    ]);
    expect(jobs.published.single['draft'], {'id': draftId, 'digest': digest});
    expect(jobs.row('j1')['state'], 'done');
    // The worker's news reached the UI as effects.
    expect(said.whereType<KeepAlive>(), isNotEmpty);
  });
}

const draftId = 'd0000000000a';
const digest = 'sha256:0123';

/// The worker's agent: turn 1 renders the draft, turn 2 finishes. Any
/// other turn, any tool call and any question fail the worker loudly.
WorkerAgent scriptedAgent(WorkerContext context) => (
  agent: _ScriptedAgent(),
  probeClaude: () async => true,
  session: () async => throw StateError('the scripted make asks nothing'),
);

final class _ScriptedAgent implements AgentPort {
  @override
  Future<TurnOutcome> turn(
    JobId jobId,
    TurnInput input,
    Future<void> cancelled,
  ) async => switch (input.turn) {
    1 => const TurnOutcome(
      TurnNotes(
        calls: [
          RenderCall(
            'r1',
            RenderSpec(draftId: draftId, revision: 1, digest: digest),
          ),
        ],
      ),
    ),
    2 when input.results['r1']?.isError == false => TurnOutcome(
      TurnNotes(
        calls: [
          FinishCall(
            'f1',
            PublishPlan(
              base: input.base,
              data: const {
                'artifact_id': null,
                'request': {
                  'title': 'Todo',
                  'files': {
                    'index.html': {'content': '<title>Todo</title>'},
                  },
                },
                'draft': {'id': draftId, 'digest': digest},
              },
            ),
          ),
        ],
      ),
    ),
    _ => throw StateError('no turn ${input.turn} scripted: $input'),
  };

  @override
  Future<ToolResult> tool(
    JobId jobId,
    int turn,
    OtherCall call,
    Future<void> cancelled,
  ) => throw StateError('the scripted make calls no tool: $call');

  /// The draft as the script names it.
  @override
  Future<RenderSpec> renderSpec(
    JobId jobId,
    int turn,
    String callId,
    RenderSpec planned,
  ) async => planned;

  @override
  Future<PublishPlan> publishPlan(
    JobId jobId,
    int turn,
    String callId,
    PublishPlan planned,
  ) async => planned;
}

/// Waits for [client]'s state to satisfy [ok].
Future<QueueState> stateWhere(
  QueueClient client,
  bool Function(QueueState s) ok,
) {
  final state = client.state;
  if (ok(state.value)) return Future.value(state.value);
  final c = Completer<QueueState>();
  void listen() {
    if (!c.isCompleted && ok(state.value)) {
      c.complete(state.value);
      state.removeListener(listen);
    }
  }

  state.addListener(listen);
  return c.future;
}
