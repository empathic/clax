/// The render boundary between the worker and the UI isolate, carried by the
/// queue's one versioned codec (`codec.dart`).
///
/// The worker holds a [RenderPort]: the effect runner hands it a
/// [RenderRequest] for each `Render` effect and awaits the
/// [RenderResult]. The UI isolate holds a [RenderEndpoint], which a
/// [RenderServer] (`RenderService`, lib/ui/render_service.dart) serves:
/// it hears [UiRender] and [UiRenderCancel], answers each render, and says
/// whether a WebView can be made now (the worker's `CanRender`).
///
/// On the wire a request or a cancel is a [ToUi] envelope, an answer a
/// [ToWorker] `WorkerRender` and the news a `WorkerMsg(CanRender)`: the
/// render boundary is part of the isolate wire, not a format of its own.
/// [WireRenderPort] and [WireRenderEndpoint] put either end over any
/// carrier of those envelopes; [inProcessRender] joins the two in one
/// isolate through the codec, as across isolates.
///
/// The worker's clock alone decides that a render took too long (its
/// 60 s deadline and one retry are `update`'s): the UI isolate never times
/// a request out.
library;

import 'dart:async';

import 'codec.dart';
import 'model.dart';
import 'msg.dart';

/// The worker's end of the render boundary: what the effect runner calls.
abstract interface class RenderPort {
  /// Asks the UI isolate to render [request]; completes with its answer,
  /// [RenderAnswered] or [RenderGone]. Never completes with an error.
  Future<RenderResult> render(RenderRequest request);

  /// Ends every render of job [jobId] (on `StopWork`): each pending one
  /// completes [RenderGone], and the UI isolate stops drawing it.
  void cancel(JobId jobId);
}

/// The UI isolate's end of the render boundary, which a [RenderServer]
/// serves.
abstract interface class RenderEndpoint {
  /// The worker's requests and cancels, in the order sent.
  Stream<UiRenderAsk> get asks;

  /// Answers a request.
  void answer(RenderResult result);

  /// Tells the worker whether a WebView can be made now (`CanRender`).
  void canRender(bool can);
}

/// What answers a worker's renders: `RenderService`.
abstract interface class RenderServer {
  /// Answers [end]'s requests and cancels, and tells it whether a render
  /// can be made, at once and on every change, until the serving is
  /// closed; closing ends the renders it asked for.
  RenderServing serve(RenderEndpoint end);
}

/// One [RenderServer.serve]: [close] it when its endpoint goes (the
/// worker it answered is gone).
abstract interface class RenderServing {
  void close();
}

/// The worker's end over the codec: [post] carries each [UiRenderAsk] to
/// the UI isolate, and the worker's inbox hands each `WorkerRender`'s
/// result to [answer]. An answer to a render no longer pending (cancelled,
/// or answered) is dropped.
final class WireRenderPort implements RenderPort {
  WireRenderPort(this._post);

  final void Function(UiRenderAsk ask) _post;
  final Map<EffectId, (JobId, Completer<RenderResult>)> _pending = {};
  bool _closed = false;

  @override
  Future<RenderResult> render(RenderRequest request) {
    if (_closed) return Future.value(RenderGone(request.effect, request.jobId));
    final done = Completer<RenderResult>();
    _pending[request.effect]?.$2.complete(
      RenderGone(request.effect, request.jobId),
    );
    _pending[request.effect] = (request.jobId, done);
    _post(UiRender(request));
    return done.future;
  }

  @override
  void cancel(JobId jobId) {
    for (final e in [..._pending.entries]) {
      if (e.value.$1 == jobId) {
        _pending.remove(e.key);
        e.value.$2.complete(RenderGone(e.key, jobId));
      }
    }
    if (!_closed) _post(UiRenderCancel(jobId));
  }

  /// The UI isolate's answer to a request.
  void answer(RenderResult result) =>
      _pending.remove(result.effect)?.$2.complete(result);

  /// Every pending render is answered gone; later ones are gone at once.
  void close() {
    _closed = true;
    for (final e in [..._pending.entries]) {
      e.value.$2.complete(RenderGone(e.key, e.value.$1));
    }
    _pending.clear();
  }
}

/// The UI isolate's end over the codec: [post] carries each answer and
/// news to the worker as a [ToWorker], and the UI isolate hands each
/// [UiRenderAsk] the worker sent to [take].
final class WireRenderEndpoint implements RenderEndpoint {
  WireRenderEndpoint(this._post);

  final void Function(ToWorker message) _post;
  final StreamController<UiRenderAsk> _asks = StreamController.broadcast();

  @override
  Stream<UiRenderAsk> get asks => _asks.stream;

  /// A request or cancel from the worker.
  void take(UiRenderAsk ask) {
    if (!_asks.isClosed) _asks.add(ask);
  }

  @override
  void answer(RenderResult result) => _post(WorkerRender(result));

  @override
  void canRender(bool can) => _post(WorkerMsg(CanRender(can)));

  /// No more asks are taken.
  Future<void> close() => _asks.close();
}

/// Both ends joined in this isolate, each message encoded and decoded by
/// the codec as across isolates and delivered asynchronously in order.
/// The endpoint's `CanRender` news, a message for the worker's loop and
/// not for the port, goes to [toLoop].
(WireRenderPort, WireRenderEndpoint) inProcessRender({
  void Function(Msg msg)? toLoop,
}) {
  late final WireRenderPort port;
  late final WireRenderEndpoint end;
  port = WireRenderPort((ask) {
    final wire = encodeToUi(ask);
    scheduleMicrotask(() {
      switch (decodeToUi(wire)) {
        case DecodeOk(value: final UiRenderAsk a):
          end.take(a);
        case final other:
          throw StateError('not a render ask: $other');
      }
    });
  });
  end = WireRenderEndpoint((m) {
    final wire = encodeToWorker(m);
    scheduleMicrotask(() {
      switch (decodeToWorker(wire)) {
        case DecodeOk(value: WorkerRender(:final result)):
          port.answer(result);
        case DecodeOk(value: WorkerMsg(:final msg)):
          toLoop?.call(msg);
        case final other:
          throw StateError('not a render answer: $other');
      }
    });
  });
  return (port, end);
}
