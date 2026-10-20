/// The UI isolate's side of rendering asked by the worker (spec
/// 2026-10-19-in-process-core-design §7.3): it answers each
/// [RenderRequest] with a [RenderResult] by its [Inspector], and tells the
/// worker whether a WebView can be made.
library;

import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import '../agent/inspector.dart';
import '../core/origins.dart';
import '../queue/codec.dart'
    show
        RenderAnswered,
        RenderGone,
        RenderRequest,
        RenderResult,
        UiRender,
        UiRenderCancel;
import '../queue/model.dart' show EffectId, JobId, RenderKind, RenderOutcome;
import '../queue/render.dart';
import 'inspector_surface.dart' show InspectorLost;

/// Renders drafts for the worker's jobs.
///
/// Each request loads the draft in the shell's inspect view
/// (`<shell>/a/<draftId>?app=1&inspect=1`, the shell's origin from
/// [origins]) in [inspector], which serves one call at a time, and is
/// answered:
///
/// - with what the page showed (its screenshot as base64 in
///   `inspection.png`), its tests found or its migration did, and what
///   broke (`errors`, `failed_tests`; see [RenderOutcome]);
/// - with a failed outcome (`{'error': message}`) when the page did not
///   load or answer, which is the draft's to fix;
/// - gone, when no WebView could be made ([canRender] false), the app went
///   away while it rendered ([available] turning false: the render is
///   stopped), the inspector's surface was torn down ([InspectorLost]), a
///   request for the same call came again, or the job's renders were
///   cancelled.
///
/// The service keeps no time of its own: the worker's deadline decides
/// that a render took too long.
class RenderService implements RenderServer {
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
    if (!_canRender.value) return RenderGone(r.effect, r.jobId);
    for (final other in _running.values.toList()) {
      if (other.request.jobId == r.jobId && other.request.callId == r.callId) {
        other.lose();
      }
    }
    final running = _Running(r);
    _running[r.effect] = running;
    try {
      final outcome = await _draw(r, running.stopped.future);
      return running.lost
          ? RenderGone(r.effect, r.jobId)
          : RenderAnswered(r.effect, r.jobId, outcome);
    } on InspectorLost {
      return RenderGone(r.effect, r.jobId);
    } on Object catch (e) {
      if (running.lost) return RenderGone(r.effect, r.jobId);
      // For the agent, as the render tool reports it; no person reads it.
      final why = e is InspectorError ? e.message : '$e'; // copy-check: allow
      return RenderAnswered(r.effect, r.jobId, _failed(why));
    } finally {
      if (identical(_running[r.effect], running)) _running.remove(r.effect);
    }
  }

  /// Ends every render of job [jobId]; each is answered gone.
  void cancel(JobId jobId) {
    for (final r in _running.values.toList()) {
      if (r.request.jobId == jobId) r.lose();
    }
  }

  @override
  RenderServing serve(RenderEndpoint end) {
    final asked = <EffectId>{};
    var last = _canRender.value;
    end.canRender(last);
    void news() {
      if (_canRender.value == last) return;
      last = _canRender.value;
      end.canRender(last);
    }

    _canRender.addListener(news);
    var open = true;
    final sub = end.asks.listen((a) {
      switch (a) {
        case UiRender(:final request):
          asked.add(request.effect);
          unawaited(
            render(request).then((result) {
              asked.remove(request.effect);
              if (open) end.answer(result);
            }),
          );
        case UiRenderCancel(:final jobId):
          cancel(jobId);
      }
    });
    return _Serving(() {
      if (!open) return;
      open = false;
      if (!_disposed) _canRender.removeListener(news);
      unawaited(sub.cancel());
      for (final id in asked) {
        _running[id]?.lose();
      }
    });
  }

  Future<RenderOutcome> _draw(RenderRequest r, Future<void> stopped) async {
    final spec = r.spec;
    final url = draftUrl(spec.draftId);
    switch (spec.kind) {
      case RenderKind.inspect:
        final i = await inspector.inspect(
          url,
          settle: Duration(milliseconds: r.settleMs),
          maxBytes: r.maxBytes,
          cancelled: stopped,
        );
        return RenderOutcome(
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
              'png': base64Encode(i.png),
            },
            'errors': [for (final e in i.uncaught) e.text],
            'failed_tests': const <String>[],
          },
        );
      case RenderKind.tests:
        if (spec.files.isEmpty) {
          return _failed('the request names no test files');
        }
        final t = await inspector.runTests(
          url,
          files: spec.files,
          cancelled: stopped,
        );
        return RenderOutcome(
          clean: t.failed.isEmpty,
          data: {
            'tests': t.toJson(),
            'errors': const <String>[],
            'failed_tests': [for (final f in t.failed) f.name],
          },
        );
      case RenderKind.migration:
        final m = spec.migration;
        if (m == null) return _failed('the request names no migration');
        final run = await inspector.runMigration(
          url,
          file: m.file,
          from: m.from,
          to: m.to,
          cancelled: stopped,
        );
        return RenderOutcome(
          clean: run.ok,
          data: {
            'migration': {'ok': run.ok, 'error': ?run.error},
            'errors': [
              if (!run.ok) run.error ?? 'the migration failed',
            ],
            'failed_tests': const <String>[],
          },
        );
    }
  }

  static RenderOutcome _failed(String why) => RenderOutcome(
    clean: false,
    data: {
      'error': why,
      'errors': [why],
      'failed_tests': const <String>[],
    },
  );

  /// The shell's inspect view of draft [draftId], on the app's origins.
  Uri draftUrl(String draftId) => Uri.parse(
    origins.shell,
  ).replace(pathSegments: ['a', draftId], query: 'app=1&inspect=1');

  /// Ends every render (each answered gone); later requests are gone.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _available?.removeListener(_onAvailable);
    _onAvailable();
    _canRender.dispose();
  }
}

final class _Serving implements RenderServing {
  _Serving(this._close);
  final void Function() _close;
  @override
  void close() => _close();
}

/// A render in progress, and whether it was lost.
final class _Running {
  _Running(this.request);
  final RenderRequest request;
  final stopped = Completer<void>();
  var lost = false;

  /// Stops the render; it is answered gone.
  void lose() {
    lost = true;
    if (!stopped.isCompleted) stopped.complete();
  }
}
