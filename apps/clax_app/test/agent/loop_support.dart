import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/timings.dart';

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
