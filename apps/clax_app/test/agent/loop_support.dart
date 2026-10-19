import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/tools/toolbox.dart';

/// A [ScriptedBackend] under another [name], whose calls can be held at a
/// gate until the test opens it, and which honours
/// [AgentRequest.cancelled] while held.
class HeldBackend implements AgentBackend {
  HeldBackend(List<Object> script, {this.name = 'scripted'})
    : _inner = ScriptedBackend(script);

  final ScriptedBackend _inner;

  @override
  final String name;

  /// Calls that start while this is set wait for it to complete.
  Completer<void>? gate;

  /// Completes when a call reaches the gate.
  Completer<void> arrived = Completer<void>();

  /// Every request, in order, recorded when its call starts.
  final List<AgentRequest> seen = [];

  @override
  Future<EditResult> run(AgentRequest request) async {
    seen.add(request);
    final g = gate;
    if (g != null) {
      if (!arrived.isCompleted) arrived.complete();
      final watch = CancelWatch(request.cancelled);
      await Future.any([g.future, watch.whenCancelled]);
      watch.check();
    }
    return _inner.run(request);
  }
}

/// Timings that keep every started and stopped name.
class RecordingTimings implements Timings {
  final List<String> started = [];
  final List<TimingRecord> stopped = [];

  @override
  void start(String name) => started.add(name);

  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) {
    if (!started.contains(name)) return null;
    final r = TimingRecord(
      name: name,
      elapsed: Duration.zero,
      at: DateTime.utc(2026, 10, 12),
      extra: extra,
    );
    stopped.add(r);
    return r;
  }

  @override
  List<TimingRecord> recent(int n) => stopped.reversed.take(n).toList();
}

/// A change result.
EditResult change({
  String reply = 'Done.',
  String? title,
  List<FileWrite> files = const [],
  List<TextEdit> edits = const [],
  List<String> removed = const [],
  List<DbOp> db = const [],
  Map<String, Object?>? capabilities,
  bool resolve = true,
}) => EditResult(
  reply: reply,
  title: title,
  files: files,
  edits: edits,
  removed: removed,
  db: db,
  capabilities: capabilities,
  resolve: resolve,
);

/// An edit of index.html.
TextEdit edit(String oldStr, String newStr, {String path = 'index.html'}) =>
    TextEdit(path: path, oldStr: oldStr, newStr: newStr);

/// Turns the event loop until [condition] holds; fails after [turns] turns
/// without it. Every step of the code under test is a fake call that
/// completes in a turn or two, so the bound is never reached by a working
/// implementation.
Future<void> until(
  bool Function() condition, {
  String? reason,
  int turns = 2000,
}) async {
  for (var i = 0; i < turns; i++) {
    if (condition()) return;
    await Future<void>.delayed(Duration.zero);
  }
  throw StateError('condition not reached: ${reason ?? condition}');
}

/// A [ScriptedRunner] of [runs] whose runs can be held at a gate until the
/// test opens it, and which ends a held run cancelled when its cancel
/// fires; it keeps every run's input and state.
class HeldRunner implements AgentRunner {
  HeldRunner(
    List<List<ScriptedCall>> runs, {
    this.name = 'scripted',
    this.sendsImages = true,
  }) : _inner = ScriptedRunner.runs(runs, name: name, sendsImages: sendsImages);

  final ScriptedRunner _inner;

  @override
  final String name;

  @override
  final bool sendsImages;

  /// Runs that start while this is set wait for it to complete.
  Completer<void>? gate;

  /// Completes when a run reaches the gate.
  Completer<void> arrived = Completer<void>();

  /// Every run's input, in order, recorded when it starts.
  final List<RunInput> seen = [];

  /// Every run's state, in order.
  final List<RunState> states = [];

  /// The text of run [i]'s first message.
  String text(int i) => [
    for (final b in seen[i].first)
      if (b['type'] == 'text') b['text']! as String,
  ].join('\n');

  @override
  Future<RunOutcome> run(RunInput input, Toolbox toolbox) async {
    seen.add(input);
    states.add(toolbox.state);
    final g = gate;
    if (g != null) {
      if (!arrived.isCompleted) arrived.complete();
      final watch = CancelWatch(toolbox.state.ctx.cancelled);
      await Future.any([g.future, watch.whenCancelled]);
      if (watch.isCancelled) {
        return toolbox.state.outcome ??= const RunCancelled();
      }
    }
    return _inner.run(input, toolbox);
  }
}

/// A change's tool calls: each of [edits] as a `str_replace` of
/// index.html, the [db] writes staged, a render, then `finish` with
/// [reply].
List<ScriptedCall> changeCalls({
  String reply = 'Done.',
  List<(String, String)> edits = const [],
  List<DbOp> db = const [],
  bool resolve = true,
  String? narration,
}) => [
  for (final (i, (from, to)) in edits.indexed)
    ScriptedCall('str_replace', {
      'path': 'index.html',
      'old_str': from,
      'new_str': to,
    }, i == 0 ? narration : null),
  if (db.isNotEmpty)
    ScriptedCall('db_write', {
      'app': null,
      'ops': [
        for (final o in db)
          {
            'op': o.op,
            'collection': o.collection,
            'doc_id': o.docId,
            'data_json': o.data == null ? null : jsonEncode(o.data),
          },
      ],
    }),
  if (edits.isNotEmpty || db.isNotEmpty)
    const ScriptedCall('render', {'file': null}),
  ScriptedCall('finish', {'reply': reply, 'resolve': resolve}),
];

/// An app icon `write_file` takes.
const iconSvg =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
    '<rect width="1024" height="1024" fill="#2a6"/></svg>';

/// A new app's tool calls: `set_meta` with [title] (and [capabilities]),
/// `write_file` of index.html with [page], a render, `write_file` of
/// icon.svg ([iconSvg]), then `finish`.
List<ScriptedCall> newAppCalls({
  String title = 'Runs',
  String page = '<!doctype html><title>Runs</title><h1>Runs</h1>',
  Map<String, Object?>? capabilities,
  String reply = 'Made a run tracker.',
  String? narration = 'Writing the tracker',
}) => [
  ScriptedCall('set_meta', {
    'title': title,
    'capabilities_json': capabilities == null ? null : jsonEncode(capabilities),
  }, narration),
  ScriptedCall('write_file', {'path': 'index.html', 'content': page}),
  const ScriptedCall('render', {'file': null}),
  const ScriptedCall('write_file', {'path': 'icon.svg', 'content': iconSvg}),
  ScriptedCall('finish', {'reply': reply, 'resolve': false}),
];
