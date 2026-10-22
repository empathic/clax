/// The production [WorkerAgentFactory]: the worker isolate's agent built
/// from lib/agent's parts over the worker's own attachment to the core.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/services.dart' show CachingAssetBundle;

import '../agent/edit_result.dart' show DbOp;
import '../agent/failure.dart' show changeCapUsd;
import '../agent/loop.dart' show anchorLine;
import '../agent/messages_api_backend.dart';
import '../agent/plan.dart';
import '../agent/prompt.dart';
import '../agent/request.dart';
import '../agent/runner.dart' show RunInput;
import '../agent/runner/turn.dart' show TurnBackend;
import '../agent/tools/budget.dart';
import '../agent/tools/data_tools.dart';
import '../agent/tools/draft.dart';
import '../agent/tools/file_tools.dart';
import '../agent/tools/kit_tool.dart';
import '../agent/tools/toolbox.dart';
import '../bundle/content_hash.dart' show sha256Hex;
import '../clax/api.dart';
import '../clax/http_api.dart';
import '../core/core_transport.dart';
import '../platform/clock.dart';
import '../platform/connection.dart' show canConnect;
import '../platform/settings_store.dart' show AppSettings;
import 'agent_port.dart';
import 'host.dart';
import 'model.dart';
import 'runner.dart' show jobOf;
import 'worker_main.dart' show WorkerContext;

/// What the worker's agent needs that only the UI isolate can read (the
/// app's bundled assets) or knows: the tool loop's system prompt
/// (`PromptBuilder.toolSystem`), the kit's files by name (for `add_kit`),
/// and the install's ID and process, which find the app's session.
final class WorkerAgentAssets {
  const WorkerAgentAssets({
    required this.system,
    required this.kit,
    required this.installId,
    required this.pid,
  });

  final String system;
  final Map<String, String> kit;
  final String installId;
  final int pid;

  /// Reads them in the UI isolate: the system prompt from [prompts], and
  /// every file [kitParts] names through [kitFile].
  static Future<WorkerAgentAssets> load(
    PromptBuilder prompts, {
    required String installId,
    required int pid,
    KitLoader kitFile = bundledKit,
  }) async {
    final names = {for (final files in kitParts.values) ...files};
    final kit = <String, String>{};
    for (final n in names) {
      kit[n] = await kitFile(n);
    }
    return WorkerAgentAssets(
      system: await prompts.toolSystem(),
      kit: kit,
      installId: installId,
      pid: pid,
    );
  }
}

/// The worker's agent of the app: [AgentTurns] over the Messages API
/// with the worker's settings and key (or [backend]'s, for a test's
/// scripted model), each job's prompt and tools as lib/agent's makes and
/// changes have them, its drafts served through the core, Claude's
/// reachability probed as the app probes it (or by [probe]), and the
/// app's session (`POST /api/sessions` by the install's ID).
///
/// It is sendable when [backend] and [probe] are top-level or static
/// functions: the worker isolate is spawned with it.
WorkerAgentFactory workerAgent(
  WorkerAgentAssets assets, {
  TurnBackend Function(WorkerContext context)? backend,
  Future<bool> Function()? probe,
}) => (context) => _agentOf(context, assets, backend, probe);

WorkerAgent _agentOf(
  WorkerContext context,
  WorkerAgentAssets assets,
  TurnBackend Function(WorkerContext context)? backendOf,
  Future<bool> Function()? probe,
) {
  final start = context.start;
  final transport = CoreTransport(context.core);
  final api = HttpClaxApi.over(transport);
  final settings = AppSettings.fromJson(start.settings);
  final key = start.apiKey;
  final backend =
      backendOf?.call(context) ?? MessagesApiBackend(apiKey: () async => key);
  Future<String>? sid;
  String? held;
  Future<String> session() => sid ??= () async {
    try {
      final s = await api.registerAppSession(
        installId: assets.installId,
        pid: assets.pid,
        cwd: start.supportDir,
      );
      return held = s.id;
    } on Object {
      sid = null;
      rethrow;
    }
  }();
  // The session ended under the worker: the next ask registers again.
  void sessionEnded(String ended) {
    if (held != ended) return;
    held = null;
    sid = null;
  }

  final jobs = WorkerJobs(
    api: api,
    transport: transport,
    assets: assets,
    model: settings.model,
    clock: start.clock,
  );
  return (
    agent: AgentTurns(
      backend: backend,
      open: jobs.open,
      serveDraft: jobs.serve,
      session: session,
    ),
    probeClaude:
        probe ??
        () => canConnect(Uri.parse(MessagesApiBackend.defaultBaseUrl).host),
    session: session,
    sessionEnded: sessionEnded,
  );
}

/// Opens the worker's jobs and serves their drafts, over the core.
///
/// A job is opened from its row (`GET /api/jobs/{id}`): a make's prompt
/// is `PromptBuilder.firstNew` with the person's other apps that keep
/// documents by ID and title, a change's `firstChange` with the app's
/// files and documents at the job's base and the comments its threads
/// answer. Its tools are lib/agent's: the plan tools, `add_kit`, the file
/// and data tools, and the queue's own ([queueTools]); its caps a make's
/// first run's (`RunCaps.first`) or a change's, its runs together within
/// `buildCapUsd` or `changeCapUsd`. Plans are kept in this worker
/// ([PlanStore] in memory): a plan written before a restart is written
/// again when the job's turns are run again.
///
/// A draft is served as `RenderTool` serves it: created on its first
/// render (`POST /api/drafts`, of the change's app), then published with
/// what changed, and the staged document writes not yet written applied
/// to its copy, best effort.
final class WorkerJobs {
  WorkerJobs({
    required this.api,
    required this.transport,
    required this.assets,
    required this.model,
    required this.clock,
  });

  final ClaxApi api;
  final CoreTransport transport;
  final WorkerAgentAssets assets;
  final String model;
  final Clock clock;

  final _plans = PlanStore();
  final _prompts = PromptBuilder(_NoAssets());
  final Map<JobId, String?> _apps = {};
  final Map<JobId, _Drafted> _drafts = {};

  /// Job [jobId]'s agent.
  Future<AgentJob> open(JobId jobId) async {
    final row = await _row(jobId);
    _apps[jobId] = row.artifactId;
    return switch (row.kind) {
      JobKind.make => _make(row),
      JobKind.change => _change(row),
    };
  }

  Future<Job> _row(JobId id) async {
    final a = await transport.send(
      'GET',
      '/api/jobs/${Uri.encodeComponent(id)}',
      timeout: const Duration(seconds: 35),
    );
    final json = a.body.isEmpty ? null : jsonDecode(utf8.decode(a.body));
    final job = a.status == 200 && json is Map ? jobOf(json['job']) : null;
    if (job == null) {
      throw JobUnopened('job $id could not be read (${a.status})');
    }
    return job;
  }

  List<Tool> _tools(RunPlan plan) => [
    PlanSetTool(plan),
    PlanMarkTool(plan),
    AddKitTool(load: _kit),
    ...fileTools(),
    ...dataTools(api),
    ...queueTools(),
  ];

  Future<String> _kit(String name) async =>
      assets.kit[name] ?? (throw JobUnopened('the kit has no $name'));

  Toolbox _toolbox(RunContext ctx, Draft draft, RunBudget budget, RunPlan plan) =>
      Toolbox(RunState(ctx: ctx, draft: draft, budget: budget), _tools(plan));

  Future<AgentJob> _make(Job row) async {
    final request = row.input.request;
    final others = await _others();
    final ctx = RunContext(
      kind: RequestKind.newApp,
      files: const {},
      clock: clock,
      request: request,
      asksIcon: true,
    );
    final plan = RunPlan(_plans, row.id, request: request);
    final draft = Draft(files: const {}, baseVersion: 0);
    final caps = _within(RunCaps.first, buildCapUsd);
    return AgentJob(
      input: RunInput(
        system: assets.system,
        first: _prompts.firstNew(request, others),
      ),
      toolbox: _toolbox(ctx, draft, RunBudget(caps, model, clock), plan),
      row: row,
      capUsd: buildCapUsd,
      again: KeepGoingRun(
        first: (note, files) => _prompts.firstNew(
          request,
          others,
          keepGoing: note,
          files: files,
        ),
        toolbox: (draft, budget) => _toolbox(ctx, draft, budget, plan),
      ),
    );
  }

  Future<AgentJob> _change(Job row) async {
    final aid =
        row.artifactId ?? (throw JobUnopened('change ${row.id} names no app'));
    final detail = await api.getArtifact(aid);
    final app = detail.artifact;
    final base = row.input.base > 0 ? row.input.base : app.currentVersion;
    final files = await api.readFiles(aid, base);
    final carried = await _carried(aid, base, files);
    final threads = await api.threads(aid);
    final documents = await _documents(aid);
    final others = await _others(exclude: aid);
    final comments = <CommentContext>[
      for (final t in threads)
        if (row.input.threads[t.id] case final newest?)
          ..._commentsOf(t, newest),
    ];
    final ctx = RunContext(
      kind: RequestKind.change,
      artifactId: aid,
      baseVersion: base,
      title: app.title,
      icon: app.icon,
      capabilities: app.capabilities,
      files: files,
      threadIds: [...row.input.threads.keys],
      shownDocs: {
        for (final c in PromptBuilder.trimDocuments(documents))
          for (final d in c.docs) d.path: d.version,
      },
      clock: clock,
      changeSpentUsd: 0,
    );
    final plan = RunPlan(_plans, aid, request: app.title, freshBuild: true);
    final draft = Draft(
      files: files,
      title: app.title,
      capabilities: app.capabilities,
      baseVersion: base,
    );
    List<Map<String, Object?>> first({
      KeepGoingNote? note,
      Map<String, String>? now,
    }) => _prompts.firstChange(
      title: app.title,
      files: now ?? files,
      comments: comments,
      others: others,
      documents: documents,
      keepGoing: note,
    );
    final caps = _within(RunCaps.change, changeCapUsd);
    return AgentJob(
      input: RunInput(system: assets.system, first: first()),
      toolbox: _toolbox(ctx, draft, RunBudget(caps, model, clock), plan),
      row: row,
      capUsd: changeCapUsd,
      carried: carried,
      again: KeepGoingRun(
        first: (note, now) => first(note: note, now: now),
        toolbox: (draft, budget) => _toolbox(ctx, draft, budget, plan),
      ),
    );
  }

  /// The files of version [n] of app [aid] that [texts] (the draft's
  /// files at its start) leave out, by hash and size: what a publish of
  /// the draft carries. A file the version's manifest names without its
  /// hash is read and hashed.
  Future<Map<String, FileHash>> _carried(
    String aid,
    int n,
    Map<String, String> texts,
  ) async {
    final path = '/api/artifacts/${Uri.encodeComponent(aid)}/versions/$n';
    final a = await transport.send(
      'GET',
      path,
      timeout: const Duration(seconds: 35),
    );
    final json = a.body.isEmpty ? null : jsonDecode(utf8.decode(a.body));
    final files = switch (json) {
      {'version': {'files': final Map f}} when a.status == 200 => f,
      _ => throw JobUnopened(
        'version $n of $aid could not be read (${a.status})',
      ),
    };
    final out = <String, FileHash>{};
    for (final MapEntry(:key, :value) in files.entries) {
      if (key is! String || texts.containsKey(key) || value is! Map) continue;
      if (value case {'sha256': final String h, 'size': final int size}) {
        out[key] = (sha256: h, size: size);
        continue;
      }
      final bytes =
          await api.readFileBytes(aid, n, key) ??
          (throw JobUnopened('$key of version $n of $aid could not be read'));
      out[key] = (sha256: sha256Hex(bytes), size: bytes.length);
    }
    return out;
  }

  /// The viewers' comments of thread [t] since the agent's last reply,
  /// up to [newest].
  static List<CommentContext> _commentsOf(ThreadView t, String newest) {
    final out = <CommentContext>[];
    for (final c in t.comments) {
      if (c.authorKind == 'agent') {
        out.clear();
        continue;
      }
      out.add(
        CommentContext(
          threadId: t.id,
          author: c.authorName,
          body: c.body,
          anchorLine: anchorLine(t.anchor, t.versionN),
        ),
      );
      if (c.id == newest) break;
    }
    return out;
  }

  static RunCaps _within(RunCaps caps, double capUsd) => RunCaps(
    turns: caps.turns,
    renders: caps.renders,
    outputTokens: caps.outputTokens,
    costUsd: capUsd < caps.costUsd ? capUsd : caps.costUsd,
    wallClock: caps.wallClock,
  );

  /// The person's other apps that keep documents, by ID and title.
  Future<List<OtherApp>> _others({String? exclude}) async {
    try {
      return [
        for (final a in await api.listArtifacts())
          if (a.id != exclude && a.declaresDb && a.kind == 'html')
            OtherApp(id: a.id, title: a.title, collections: const []),
      ].take(PromptBuilder.maxOthers).toList();
    } on ClaxApiError {
      return const [];
    }
  }

  /// App [aid]'s collections with their first documents, as a change's
  /// first message shows them; none when they cannot be read.
  Future<List<AppCollection>> _documents(String aid) async {
    try {
      final colls = await api.collections(aid);
      return [
        for (final c in colls.take(PromptBuilder.maxOwnCollections))
          (
            path: c.path,
            count: c.count,
            docs: <AppDocument>[
              for (final d in await api.listDocs(
                aid,
                c.path,
                limit: PromptBuilder.maxOwnDocs,
              ))
                (path: d.path, version: d.version, data: d.data),
            ],
          ),
      ];
    } on ClaxApiError {
      return const [];
    }
  }

  /// Serves [snap] of job [jobId]'s draft: its draft artifact's ID.
  Future<String> serve(JobId jobId, DraftSnapshot snap) async {
    var held = _drafts[jobId];
    if (held != null && held.base != snap.baseVersion) {
      await _forget(held.id);
      held = null;
    }
    if (held != null) {
      try {
        await _publish(held, snap);
      } on ClaxApiError catch (e) {
        // The core swept the draft: serve a fresh one.
        if (!e.isNotFound) rethrow;
        held = null;
      }
    }
    held ??= _drafts[jobId] = await _create(jobId, snap);
    await _writeDocs(held, snap.stagedDb);
    return held.id;
  }

  Future<_Drafted> _create(JobId jobId, DraftSnapshot snap) async {
    final title = switch (snap.title) {
      final t? when t.trim().isNotEmpty => t,
      _ => 'New app',
    };
    final info = await api.createDraft(
      of: _apps[jobId],
      title: title,
      capabilities: snap.capabilities,
      files: snap.files,
    );
    return _Drafted(
      info.id,
      base: snap.baseVersion,
      version: info.version ?? 1,
      files: snap.files,
      title: title,
      caps: _capsKey(snap.capabilities),
    );
  }

  Future<void> _publish(_Drafted d, DraftSnapshot snap) async {
    final files = <String, String?>{
      for (final MapEntry(:key, :value) in snap.files.entries)
        if (d.files[key] != value) key: value,
      for (final p in d.files.keys)
        if (!snap.files.containsKey(p)) p: null,
    };
    final title = snap.title != null && snap.title != d.title
        ? snap.title
        : null;
    final caps = _capsKey(snap.capabilities);
    final newCaps = snap.capabilities != null && caps != d.caps;
    if (files.isEmpty && title == null && !newCaps) return;
    PublishRequest request(int version) => PublishRequest(
      ifVersion: version,
      title: title,
      capabilities: newCaps ? snap.capabilities : null,
      // A publish needs index.html; the other unchanged files carry on.
      files: {'index.html': snap.files['index.html'], ...files},
    );
    PublishResult res;
    try {
      res = await api.publishDraft(d.id, request(d.version));
    } on ClaxApiError catch (e) {
      final current = e.current;
      if (!e.isConflict || current == null) rethrow;
      res = await api.publishDraft(d.id, request(current));
    }
    d
      ..version = res.version.n
      ..files = snap.files;
    if (title != null) d.title = title;
    if (newCaps) d.caps = caps;
  }

  /// Applies the staged writes the draft's copy has not had, one at a
  /// time; a write the core refuses is left out (the publish step's own
  /// check decides on the app's documents).
  Future<void> _writeDocs(_Drafted d, List<DbOp> staged) async {
    if (d.applied > staged.length) d.applied = 0;
    for (final o in staged.sublist(d.applied)) {
      try {
        await api.batchDocs(d.id, [
          DocWrite(
            op: o.op,
            collection: o.collection,
            docId: o.docId,
            data: o.op == 'delete' ? null : o.data ?? const {},
          ),
        ]);
      } on ClaxApiError {
        // The draft's copy goes without it.
      }
    }
    d.applied = staged.length;
  }

  Future<void> _forget(String id) async {
    try {
      await api.deleteDraft(id);
    } on ClaxApiError {
      // Gone already: the core's sweep removes drafts left behind.
    }
  }

  static String? _capsKey(Map<String, Object?>? caps) =>
      caps == null ? null : jsonEncode(caps);
}

/// What [WorkerJobs] could not open or load for a job: its row unread, a
/// change naming no app, or a kit file the worker was not given. The
/// runner answers it as the turn's or the call's failure.
final class JobUnopened implements Exception {
  const JobUnopened(this.message);
  final String message;
  @override
  String toString() => message;
}

/// A job's draft artifact and what it was last given.
final class _Drafted {
  _Drafted(
    this.id, {
    required this.base,
    required this.version,
    required this.files,
    required this.title,
    required this.caps,
  });
  final String id;
  final int base;
  int version;
  Map<String, String> files;
  String? title;
  String? caps;
  int applied = 0;
}

/// No bundle: the worker builds first messages only, which read none.
final class _NoAssets extends CachingAssetBundle {
  @override
  Future<ByteData> load(String key) =>
      throw StateError('the worker reads no asset ($key)');
}
