/// The queue's [AgentPort] over lib/agent's parts: a [TurnBackend] asks
/// the model one turn at a time, and a job's [Toolbox] runs its tool calls
/// (spec 2026-10-19-in-process-core-design §7.1: "the agent's runners and
/// backends, unchanged in substance").
library;

import 'dart:async';
import 'dart:convert';

import '../agent/backend.dart' show AgentError;
import '../agent/runner.dart' show RunInput;
import '../agent/runner/messages_runner.dart' show finishNudge;
import '../agent/runner/turn.dart';
import '../agent/tools/draft.dart' show DraftSnapshot;
import '../agent/tools/toolbox.dart';
import '../bundle/content_hash.dart' show contentHash, sha256Hex;
import 'model.dart';
import 'runner.dart' show AgentPort;

/// One job's agent in the worker: the prompt its first turn starts from,
/// and the tools its calls run on, over the job's draft.
final class AgentJob {
  AgentJob({required this.input, required this.toolbox});
  final RunInput input;
  final Toolbox toolbox;
}

/// Opens job [jobId]'s agent: its prompt and its [Toolbox] over a draft of
/// the job's app at the job's base version (the opener reads the job from
/// the core).
typedef OpenJob = Future<AgentJob> Function(JobId jobId);

/// Serves [snapshot] of job [jobId]'s draft as a draft artifact the UI
/// isolate can load (`POST /api/drafts`, then a publish of what changed):
/// its ID.
typedef ServeDraft =
    Future<String> Function(JobId jobId, DraftSnapshot snapshot);

/// The digest a render of [files] is recorded under (spec §8.2,
/// `clax_core::digest::files_digest`): the content hash of the files but
/// `icon.svg`.
String draftDigest(Map<String, String> files) => contentHash({
  for (final e in files.entries)
    if (e.key != 'icon.svg')
      e.key: (
        sha256: sha256Hex(utf8.encode(e.value)),
        size: utf8.encode(e.value).length,
      ),
});

/// A test file of a draft, as the inspect view takes it.
final _testFile = RegExp(r'^tests/[A-Za-z0-9_\-./]{1,200}\.test\.js$');

/// [AgentPort] over a [TurnBackend] and each job's [Toolbox].
///
/// A turn sends the job's history, as `MessagesApiRunner` does: the first
/// user message, then each assistant turn as it came and the results of
/// its calls in one user message, in the calls' order (a call without a
/// result answers as an error); a turn that called no tool is followed by
/// [finishNudge]. The turn's `tool_use` blocks become its calls:
///
/// - `render` → [RenderCall]; when the queue runs it, [renderSpec] serves
///   the draft as the calls before it left it ([ServeDraft]), with its
///   revision, digest and test files;
/// - `finish` → [FinishCall] at the turn's base; when the queue publishes
///   it, [publishPlan] makes `{artifact_id, request: {title, capabilities,
///   files}, draft: {id, digest}}` of the draft then, `draft` naming the
///   job's last served render (the core's render gate checks it). A
///   change's call carries its reply (`{threads, body, resolve}`);
/// - `ask` → [AskCall] with the call's input;
/// - any other → [OtherCall], which [tool] runs on the job's toolbox.
///
/// A turn whose answer was cut off, or holds a tool call that is not JSON,
/// fails (`TurnFailed(other)`); a refusal too. A turn that reached a hard
/// cap of the job's budget stops ([TurnStop], Keep going offered).
///
/// A job's history and draft live in this worker: a worker that starts at
/// turn `n > 1` (after a restart) rebuilds the history from the first
/// message and the turn before's notes alone, and the draft from the
/// opener's and the last turn's calls, run again.
final class AgentTurns implements AgentPort {
  AgentTurns({
    required this.backend,
    required this.open,
    required this.serveDraft,
    this.maxTokens = 32000,
  });

  final TurnBackend backend;
  final OpenJob open;
  final ServeDraft serveDraft;
  final int maxTokens;

  final Map<JobId, Future<_Job>> _jobs = {};

  /// The draft each job's last render served, and its digest.
  final Map<JobId, ({String draftId, String digest})> _served = {};

  /// Forgets job [jobId]'s history and tools (it ended).
  void forget(JobId jobId) {
    unawaited(_jobs.remove(jobId));
    _served.remove(jobId);
  }

  Future<_Job> _jobOf(JobId jobId) =>
      _jobs[jobId] ??= open(jobId).then(_Job.new);

  @override
  Future<TurnOutcome> turn(
    JobId jobId,
    TurnInput input,
    Future<void> cancelled,
  ) async {
    final job = await _jobOf(jobId);
    job.history(input);
    final state = job.agent.toolbox.state;
    final turn = await backend.turn(
      TurnRequest(
        model: state.budget.model,
        system: job.agent.input.system,
        tools: [for (final s in job.agent.toolbox.specs) s.toMessagesApi()],
        messages: List.unmodifiable(job.messages),
        maxTokens: maxTokens,
        outputCap: state.budget.caps.outputTokens,
        cancelled: cancelled,
        outputBefore: state.budget.usage.output,
      ),
    );
    state.budget.addTurn(turn.usage, attempts: turn.attempts);
    if (turn.stopReason == 'refusal') {
      final category = turn.stopDetails?['category'] ?? 'no category given';
      throw AgentError('The model declined: $category');
    }
    if (turn.broken || turn.stopReason == 'max_tokens') {
      throw const AgentError('the answer was cut off', retryable: true);
    }
    final change = input.input.threads.isNotEmpty;
    final calls = <ToolCall>[
      for (final b in turn.content)
        if (b['type'] == 'tool_use')
          _call(input, change, '${b['id']}', '${b['name']}', switch (b['input']) {
            final Map<String, Object?> m => m,
            _ => const <String, Object?>{},
          }),
    ];
    job.turned(input.turn, turn.content);
    return TurnOutcome(
      TurnNotes(calls: calls, data: {'assistant': turn.content}),
      stop: switch (state.budget.hardStop) {
        null => null,
        final cap => TurnStop(canKeepGoing: true, data: {'cap': cap}),
      },
    );
  }

  static ToolCall _call(
    TurnInput input,
    bool change,
    String id,
    String name,
    Map<String, Object?> args,
  ) => switch (name) {
    // The draft is served when the queue runs the call, after the calls
    // before it: the spec here names nothing yet.
    'render' => RenderCall(id, const RenderSpec(draftId: '', revision: 0)),
    'finish' => FinishCall(
      id,
      PublishPlan(base: input.base),
      reply: change
          ? ReplyPlan(
              data: {
                'threads': [...input.input.threads.keys],
                'body': args['reply'] ?? '',
                'resolve': args['resolve'] == true,
              },
            )
          : null,
    ),
    'ask' => AskCall(id, Question(data: args)),
    _ => OtherCall(id, name, args: args),
  };

  @override
  Future<RenderSpec> renderSpec(
    JobId jobId,
    int turn,
    String callId,
    RenderSpec planned,
  ) async {
    final job = await _jobOf(jobId);
    final snap = job.agent.toolbox.state.draft.snapshot();
    final digest = draftDigest(snap.files);
    final draftId = await serveDraft(jobId, snap);
    _served[jobId] = (draftId: draftId, digest: digest);
    return RenderSpec(
      draftId: draftId,
      revision: snap.revision,
      kind: planned.kind,
      digest: digest,
      files: [...snap.files.keys.where(_testFile.hasMatch)]..sort(),
      migration: planned.migration,
    );
  }

  @override
  Future<PublishPlan> publishPlan(
    JobId jobId,
    int turn,
    String callId,
    PublishPlan planned,
  ) async {
    final job = await _jobOf(jobId);
    final state = job.agent.toolbox.state;
    final draft = state.draft;
    final served = _served[jobId];
    return PublishPlan(
      base: planned.base,
      data: {
        'artifact_id': state.ctx.artifactId,
        'request': {
          'title': draft.title,
          'capabilities': ?draft.capabilities,
          'files': {
            for (final e in draft.files.entries) e.key: {'content': e.value},
          },
        },
        if (served != null)
          'draft': {'id': served.draftId, 'digest': served.digest},
      },
    );
  }

  @override
  Future<ToolResult> tool(
    JobId jobId,
    int turn,
    OtherCall call,
    Future<void> cancelled,
  ) async {
    final job = await _jobOf(jobId);
    final out = await job.agent.toolbox.call(call.name, switch (call.args) {
      final Map m => m.cast<String, Object?>(),
      _ => const <String, Object?>{},
    });
    return switch (out) {
      ToolOk(:final json, :final png) => ToolResult(
        data: {...json, if (png != null) 'png': base64Encode(png)},
      ),
      ToolRefused(:final message) => ToolResult(
        data: {'error': message},
        isError: true,
      ),
    };
  }
}

/// One job's agent and its history.
final class _Job {
  _Job(this.agent);
  final AgentJob agent;
  final List<Map<String, Object?>> messages = [];

  /// The last turn whose answer [messages] holds; 0 for none.
  int _turn = 0;

  /// Brings [messages] up to the turn [input] starts: the first message,
  /// and the turn before with its calls' results.
  void history(TurnInput input) {
    if (input.turn == 1 || messages.isEmpty || _turn != input.turn - 1) {
      messages
        ..clear()
        ..add({'role': 'user', 'content': agent.input.first});
      final before = input.previous;
      if (input.turn > 1 && before != null) {
        if (before.data case {'assistant': final List content}) {
          messages.add({'role': 'assistant', 'content': content});
        }
      }
    }
    final before = input.previous;
    if (input.turn == 1 || before == null) return;
    if (before.calls.isEmpty) {
      messages.add({
        'role': 'user',
        'content': [
          {'type': 'text', 'text': finishNudge},
        ],
      });
      return;
    }
    messages.add({
      'role': 'user',
      'content': [
        for (final c in before.calls) _result(c.id, input.results[c.id]),
      ],
    });
  }

  void turned(int turn, List<Map<String, Object?>> content) {
    _turn = turn;
    if (content.isNotEmpty) {
      messages.add({'role': 'assistant', 'content': content});
    }
  }

  static Map<String, Object?> _result(String id, ToolResult? r) {
    if (r == null) {
      return {
        'type': 'tool_result',
        'tool_use_id': id,
        'is_error': true,
        'content': [
          {'type': 'text', 'text': 'The call has no result.'},
        ],
      };
    }
    final data = r.data;
    final png = _png(data);
    return {
      'type': 'tool_result',
      'tool_use_id': id,
      if (r.isError) 'is_error': true,
      'content': [
        {'type': 'text', 'text': jsonEncode(_withoutPng(data))},
        if (png != null)
          {
            'type': 'image',
            'source': {
              'type': 'base64',
              'media_type': 'image/png',
              'data': png,
            },
          },
      ],
    };
  }

  /// A screenshot a result carries: a tool's `png`, or a render's
  /// `inspection.png`.
  static String? _png(Object? data) => switch (data) {
    {'png': final String p} => p,
    {'inspection': {'png': final String p}} => p,
    _ => null,
  };

  static Object? _withoutPng(Object? data) => switch (data) {
    final Map m => {
      for (final e in m.entries)
        if (e.key != 'png') e.key: _withoutPng(e.value),
    },
    _ => data,
  };
}

