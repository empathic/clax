/// Starts the queue's worker isolate on the open core and hands the UI its
/// [QueueClient] (spec 2026-10-19-in-process-core-design §7.1, §7.2).
library;

import '../core/core_client.dart';
import '../daemon/clock.dart';
import 'client.dart';
import 'effect.dart';
import 'isolate_client.dart';
import 'msg.dart';
import 'render.dart';
import 'runner.dart';
import 'worker_main.dart';

/// What the worker builds its agent from, in the worker isolate: the
/// [AgentPort] its turns and tools run on, the probe of Claude's
/// reachability, and the live session the `ask` tool's questions are
/// asked from.
typedef WorkerAgent = ({
  AgentPort agent,
  Future<bool> Function() probeClaude,
  Future<String> Function() session,
});

/// Builds the worker's agent from its context ([WorkerContext.start]
/// holds the settings snapshot and the key; [WorkerContext.core] is the
/// worker's own attachment to the core). It runs in the worker isolate, so
/// it must be sendable: a top-level or static function.
typedef WorkerAgentFactory = WorkerAgent Function(WorkerContext context);

/// The queue of the app: one worker isolate running `update` and the
/// [EffectRunner] on the core's jobs, restarted once if it dies, and the
/// [client] the UI reads and sends intents to.
///
/// What the app gives it, once the core is open:
///
/// - **The core.** [start] takes the open core's handle
///   (`CoreHost.handle`, the `CoreClient.handle` of the core the UI
///   isolate opened). The worker attaches to it by that handle
///   (`CoreClient.attach`, through [WorkerStart.attach]) and makes its own
///   calls and its own event subscription; the UI isolate's core is
///   untouched. The core must stay open while the host runs: a closed core
///   answers nothing, and the worker waits for its reopen (a reopened core
///   is a new handle: [dispose] this host and [start] another).
/// - **Rendering.** [render] is the app's `RenderService`
///   (lib/ui/render_service.dart) over the inspect view's
///   `WebViewInspector` and the app's lifecycle as `available` (false while
///   the UI engine is detached on Android, or the app is in the background
///   on iOS). Each worker gets an endpoint of its own, served when it is
///   spawned and closed when it ends. The host never disposes [render].
/// - **The agent.** [agent] builds the worker's [AgentPort] (lib/queue's
///   `AgentTurns` over a `TurnBackend` and each job's `Toolbox`), its
///   Claude probe and its session, in the worker isolate.
/// - **The platform's news.** [onEffect] hears the worker's `KeepAlive`
///   (start or stop the background work: `ChannelBackgroundWork` on
///   Android, `IosBackgroundWork` on iOS) and `Notice` (a job ended or
///   stopped while no view showed it). [tell] passes the app's lifecycle
///   on: `Woke(foreground:)` when it comes back, `OutOfTime` when the
///   system's time for it runs out.
///
/// Lifecycle: [start] spawns the worker at once and returns; intents sent
/// before it is up wait for it. [dispose] ends the worker (its claim on
/// the queue goes with its event stream) and closes its render endpoint;
/// the client then sends nothing. Settings and key changes reach only a
/// worker spawned after them (record 68692ab6): dispose and start again.
final class QueueHost {
  QueueHost._(this._client);

  /// Spawns the worker on the open core [coreHandle].
  static QueueHost start({
    required int coreHandle,
    required String supportDir,
    required Map<String, Object?> settings,
    required String? apiKey,
    required RenderServer render,
    required WorkerAgentFactory agent,
    void Function(Effect effect)? onEffect,
    Clock clock = const SystemClock(),
    CoreCalls Function(int handle) attach = attachCore,
  }) => QueueHost._(
    IsolateQueueClient.start(
      coreHandle: coreHandle,
      supportDir: supportDir,
      settings: settings,
      apiKey: apiKey,
      runner: _runnerOf(agent),
      clock: clock,
      attach: attach,
      render: render,
      onEffect: onEffect,
    ),
  );

  final IsolateQueueClient _client;

  /// The queue as the UI sees it: `AppServices.queue`.
  QueueClient get client => _client;

  /// Completes once the worker is up.
  Future<void> get ready => _client.ready;

  /// Passes the app's lifecycle to the worker: [Woke] or [OutOfTime].
  void tell(Msg news) => _client.tell(news);

  /// Ends the worker; nothing restarts it.
  Future<void> dispose() => _client.dispose();
}

/// The worker's runner over [agent]: a closure over a sendable function
/// only, so the worker isolate can be spawned with it.
RunnerFactory _runnerOf(WorkerAgentFactory agent) => (context) {
  final a = agent(context);
  return EffectRunner.forWorker(
    context,
    agent: a.agent,
    probeClaude: a.probeClaude,
    session: a.session,
  );
};
