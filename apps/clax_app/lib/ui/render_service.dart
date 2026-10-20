/// The UI isolate's side of rendering asked by the worker (spec
/// 2026-10-19-in-process-core-design §7.3): it answers each
/// [RenderRequest] with a [RenderResult] by its [Inspector], and tells the
/// worker whether a WebView can be made.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../agent/inspector.dart';
import '../core/origins.dart';
import '../queue/model.dart' show EffectId, JobId, RenderKind, RenderOutcome;
import '../queue/render/render_port.dart';
import 'inspector_surface.dart' show InspectorLost;

/// Renders drafts for the worker's jobs.
///
/// Each request loads the draft in the shell's inspect view
/// (`<shell>/a/<draftId>?app=1&inspect=1`, the shell's origin from
/// [origins]) in [inspector], which serves one call at a time, and is
/// answered:
///
/// - with what the page showed, its tests found or its migration did;
/// - with a failed outcome (`{'error': message}`) when the page did not
///   load or answer, which is the draft's to fix;
/// - lost, when no WebView could be made ([canRender] false), the app went
///   away while it rendered ([available] turning false: the render is
///   stopped), the inspector's surface was torn down ([InspectorLost]), a
///   request for the same call came again, or the job's renders were
///   cancelled.
///
/// The service keeps no time of its own: the worker's deadline decides
/// that a render took too long.
class RenderService {
  RenderService({
    required this.inspector,
    required this.origins,
    this._available,
  }) {
    _available?.addListener(_onAvailable);
    _onAvailable();
  }

  final Inspector inspector;
  final AppOrigins origins;

  /// Whether the app can show a WebView now; true when not given.
  final ValueListenable<bool>? _available;

  final _canRender = ValueNotifier<bool>(true);
  final _running = <EffectId, _Running>{};
  var _disposed = false;

  /// Whether a WebView can be made now: [available], and not disposed.
  ValueListenable<bool> get canRender => _canRender;

  void _onAvailable() {
    final can = !_disposed && (_available?.value ?? true);
    if (_canRender.value != can) _canRender.value = can;
    if (!can) {
      for (final r in _running.values.toList()) {
        r.lose();
      }
    }
  }

  /// Renders [r]; never throws.
  Future<RenderResult> render(RenderRequest r) async {
    if (!_canRender.value) return RenderResult.lost(r.id, r.jobId);
    for (final other in _running.values.toList()) {
      if (other.request.jobId == r.jobId && other.request.callId == r.callId) {
        other.lose();
      }
    }
    final running = _Running(r);
    _running[r.id] = running;
    try {
      final (outcome, png) = await _draw(r, running.stopped.future);
      return running.lost
          ? RenderResult.lost(r.id, r.jobId)
          : RenderResult.done(r.id, r.jobId, outcome, png: png);
    } on InspectorLost {
      return RenderResult.lost(r.id, r.jobId);
    } on Object catch (e) {
      if (running.lost) return RenderResult.lost(r.id, r.jobId);
      // For the agent, as the render tool reports it; no person reads it.
      final why = e is InspectorError ? e.message : '$e'; // copy-check: allow
      return RenderResult.done(
        r.id,
        r.jobId,
        RenderOutcome(clean: false, data: {'error': why}),
      );
    } finally {
      if (identical(_running[r.id], running)) _running.remove(r.id);
    }
  }

  /// Ends every render of job [jobId]; each is answered lost.
  void cancel(JobId jobId) {
    for (final r in _running.values.toList()) {
      if (r.request.jobId == jobId) r.lose();
    }
  }

  /// Answers [end]'s requests and cancels, and tells it [canRender] now
  /// and on every change, until the serving is closed; closing ends the
  /// renders it asked for.
  RenderServing serve(RenderEndpoint end) {
    final asked = <EffectId>{};
    var last = _canRender.value;
    end.reply(RenderNews(canRender: last));
    void news() {
      if (_canRender.value == last) return;
      last = _canRender.value;
      end.reply(RenderNews(canRender: last));
    }

    _canRender.addListener(news);
    var open = true;
    final sub = end.asks.listen((a) {
      switch (a) {
        case RenderRequest():
          asked.add(a.id);
          unawaited(
            render(a).then((result) {
              asked.remove(a.id);
              if (open) end.reply(result);
            }),
          );
        case RenderCancel(:final jobId):
          cancel(jobId);
      }
    });
    return RenderServing._(() {
      if (!open) return;
      open = false;
      if (!_disposed) _canRender.removeListener(news);
      unawaited(sub.cancel());
      for (final id in asked) {
        _running[id]?.lose();
      }
    });
  }

  Future<(RenderOutcome, Uint8List?)> _draw(
    RenderRequest r,
    Future<void> stopped,
  ) async {
    final url = draftUrl(r.draftId);
    switch (r.kind) {
      case RenderKind.inspect:
        final i = await inspector.inspect(
          url,
          settle: Duration(milliseconds: r.settleMs),
          maxBytes: r.maxBytes,
          cancelled: stopped,
        );
        return (
          RenderOutcome(
            clean: i.uncaught.isEmpty,
            data: {
              'inspection': {
                'console_errors': [
                  for (final e in i.errors) {'kind': e.kind, 'text': e.text},
                ],
                'failed_requests': [...i.failed],
                'text': i.text,
                'title': i.title,
                'height': i.height,
                'settled_ms': i.settledMs,
              },
            },
          ),
          i.png,
        );
      case RenderKind.tests:
        if (r.files.isEmpty) {
          return (_failed('the request names no test files'), null);
        }
        final t = await inspector.runTests(
          url,
          files: r.files,
          cancelled: stopped,
        );
        return (
          RenderOutcome(clean: t.failed.isEmpty, data: {'tests': t.toJson()}),
          null,
        );
      case RenderKind.migration:
        final m = r.migration;
        if (m == null) {
          return (_failed('the request names no migration'), null);
        }
        final run = await inspector.runMigration(
          url,
          file: m.file,
          from: m.from,
          to: m.to,
          cancelled: stopped,
        );
        return (
          RenderOutcome(
            clean: run.ok,
            data: {
              'migration': {'ok': run.ok, 'error': ?run.error},
            },
          ),
          null,
        );
    }
  }

  static RenderOutcome _failed(String why) =>
      RenderOutcome(clean: false, data: {'error': why});

  /// The shell's inspect view of draft [draftId], on the app's origins.
  Uri draftUrl(String draftId) =>
      Uri.parse(origins.shell)
          .replace(pathSegments: ['a', draftId], query: 'app=1&inspect=1');

  /// Ends every render (each answered lost); later requests are lost.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _available?.removeListener(_onAvailable);
    _onAvailable();
    _canRender.dispose();
  }
}

/// One [RenderService.serve]: [close] it when its endpoint goes (the
/// worker it answered is gone).
final class RenderServing {
  RenderServing._(this._close);
  final void Function() _close;
  void close() => _close();
}

/// A render in progress, and whether it was lost.
final class _Running {
  _Running(this.request);
  final RenderRequest request;
  final stopped = Completer<void>();
  var lost = false;

  /// Stops the render; it is answered lost.
  void lose() {
    lost = true;
    if (!stopped.isCompleted) stopped.complete();
  }
}
