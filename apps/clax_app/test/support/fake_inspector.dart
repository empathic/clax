import 'dart:async';
import 'dart:typed_data';

import 'package:clax_app/agent/inspector.dart';

/// One [FakeInspector.inspect] call.
class InspectCall {
  InspectCall(this.url, this.settle, this.maxBytes, this.cancelled);

  final Uri url;
  final Duration settle;
  final int maxBytes;
  final Future<void>? cancelled;
}

/// An [Inspector] that answers from a script.
///
/// Each call takes the next queued answer ([answer], [fail], [hang]), else
/// a clean inspection of [png]. A [hang] answer waits until the call's
/// `cancelled` fires (then [InspectorError] `cancelled`) or [release] ends
/// it with `did not load`, as a page that never answers.
class FakeInspector implements Inspector {
  final List<InspectCall> calls = [];
  final _queue = <Object>[];
  final _hanging = <Completer<void>>[];

  /// The screenshot of a default answer.
  final png = Uint8List.fromList([0x89, 0x50, 0x4e, 0x47]);

  /// A clean inspection of [png] with [errors].
  Inspection clean({
    List<({String kind, String text})> errors = const [],
    List<String> failed = const [],
  }) => Inspection(
    png: png,
    errors: errors,
    failed: failed,
    text: 'Runs\n[button] Add',
    title: 'Runs',
    height: 844,
    settledMs: 320,
  );

  /// Queues [i] as the next answer.
  void answer(Inspection i) => _queue.add(i);

  /// Queues [e] as the next answer, thrown.
  void fail(InspectorError e) => _queue.add(e);

  /// Queues a page that never answers.
  void hang() => _queue.add(#hang);

  /// Ends every hanging call with `did not load`.
  void release() {
    for (final c in _hanging) {
      if (!c.isCompleted) c.complete();
    }
    _hanging.clear();
  }

  @override
  Future<Inspection> inspect(
    Uri url, {
    Duration settle = const Duration(seconds: 3),
    int maxBytes = 1 << 20,
    Future<void>? cancelled,
  }) async {
    calls.add(InspectCall(url, settle, maxBytes, cancelled));
    final next = _queue.isEmpty ? clean() : _queue.removeAt(0);
    switch (next) {
      case final Inspection i:
        return i;
      case final InspectorError e:
        throw e;
      default:
        final released = Completer<void>();
        _hanging.add(released);
        final cancel = Completer<String>();
        cancelled?.then((_) {
          if (!cancel.isCompleted) cancel.complete('cancelled');
        });
        released.future.then((_) {
          if (!cancel.isCompleted) cancel.complete('did not load');
        });
        throw InspectorError(await cancel.future);
    }
  }
}
