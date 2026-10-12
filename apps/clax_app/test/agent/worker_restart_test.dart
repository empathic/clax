import 'dart:async';

import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_supervisor.dart';
import 'loop_support.dart';

const page = '<!doctype html><title>Runs</title><button>Save</button>';

const unreachable = ClaxApiError(
  status: 0,
  code: 'daemon_unreachable',
  message: 'no connection',
);

/// The feedback worker as the app runs it ([AgentRuntime]): started by the
/// supervisor's `Up`, stopped by its `Failed`, recovering cut-off changes
/// from the daemon's threads.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeSupervisor supervisor;
  late FakeClaxApi api;
  late HeldBackend backend;
  late AgentRuntime runtime;
  late List<WorkerEvent> events;

  AgentRuntime runtimeWith(List<Object> script) {
    backend = HeldBackend(script);
    final loop = AgentLoop(
      api: api,
      backend: () => backend,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
    );
    runtime = AgentRuntime(
      supervisor: supervisor,
      api: api,
      loop: loop,
      installId: 'install',
      pid: 42,
      cwd: '/support',
      delay: (_) async {},
    );
    runtime.worker.events.listen(events.add);
    runtime.attach();
    return runtime;
  }

  int registrations() => api.callNames.where((n) => n == 'registerAppSession').length;

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && runtime.worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  setUp(() {
    supervisor = FakeSupervisor();
    api = FakeClaxApi();
    events = [];
  });

  tearDown(() async {
    await runtime.close();
    api.releasePolls();
  });

  test('the worker waits for the daemon, then registers as the app session', () async {
    runtimeWith([]);
    await until(() => true);
    expect(api.callNames, isEmpty, reason: 'nothing before Up');

    supervisor.set(Up(endpoint()));
    await idle();
    final register = api.calls.firstWhere((c) => c.name == 'registerAppSession');
    expect(register.args['installId'], 'install');
    expect(register.args['pid'], 42);
    expect(register.args['cwd'], '/support');
  });

  test('worker_survives_daemon_restart_mid_change', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    runtimeWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
    ]);
    supervisor.set(Up(endpoint(pid: 1)));
    await idle();
    backend.gate = Completer<void>();
    final item = api.sendComment(aid, 'Say Keep');
    await backend.arrived.future;

    // The daemon dies while the model runs: its long poll breaks, its
    // session is gone, and the supervisor brings up a new one.
    supervisor.set(const Restarting(1, Duration(milliseconds: 200)));
    api.endSession('install');
    api.releasePolls();
    api.failNext('publish', unreachable);
    supervisor.set(Up(endpoint(pid: 2)));
    await until(() => registrations() == 2 && api.waitingPolls == 1);
    expect(backend.seen, hasLength(1), reason: 'the change in flight is kept');

    // Its publish then reaches nobody; recovery runs it again from the
    // daemon's threads.
    backend.gate!.complete();
    backend.gate = null;
    await until(() => api.currentVersion(aid) == 2);
    await idle();

    expect(api.callNames, contains('allThreads'));
    expect(backend.seen, hasLength(2), reason: 'run again exactly once');
    expect(api.currentVersion(aid), 2, reason: 'exactly one new version');
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.authorKind], ['viewer', 'agent']);
    expect(t.comments.last.body, 'Keep.');
    expect(t.status, 'resolved');
    expect(events.whereType<WorkerLanded>(), hasLength(1));
  });

  test('a failed daemon stops the worker; Restart brings it back and the '
      'cut-off change runs once', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    runtimeWith([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
    ]);
    supervisor.set(Up(endpoint(pid: 1)));
    await idle();
    backend.gate = Completer<void>();
    final item = api.sendComment(aid, 'Say Keep');
    await backend.arrived.future;

    supervisor.set(const Failed('clax stopped 5 times within a minute', []));
    await until(() => events.whereType<WorkerCancelled>().isNotEmpty);
    expect(api.callNames, isNot(contains('publish')));
    expect(api.threadOf(aid, item.threadId).comments, hasLength(1));

    api.endSession('install');
    api.releasePolls();
    backend.gate = null;
    supervisor.set(Up(endpoint(pid: 3)));
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(registrations(), 2);
    expect(api.currentVersion(aid), 2);
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.authorKind], ['viewer', 'agent']);
  });
}
