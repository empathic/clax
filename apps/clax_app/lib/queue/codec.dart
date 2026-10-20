/// The wire between the UI isolate and the worker isolate (spec
/// 2026-10-19-in-process-core-design §7.1–§7.2): every message that
/// crosses it, as a plain JSON-compatible value (maps with string keys,
/// lists, strings, numbers, booleans and null), safe to send on a
/// `SendPort` and to `jsonEncode`.
///
/// Each top-level value is an envelope `{"v": codecVersion, "t": tag, …}`.
/// Encoding is total. Decoding never throws: input of another version, an
/// unknown tag, or a value of the wrong shape answers a [DecodeFailure]
/// naming where it went wrong. A value encoded and decoded is equal to the
/// one encoded.
library;

import 'effect.dart';
import 'model.dart';
import 'msg.dart';

/// The wire's version; a decoder refuses any other.
const int codecVersion = 1;

// ------------------------------------------------------------ boundary types

/// What the worker asks the UI isolate to render: [spec], for the `Render`
/// effect [effect] of job [jobId]. Its answer is the [RenderResult] naming
/// the same [effect].
final class RenderRequest {
  const RenderRequest(this.effect, this.jobId, this.spec);
  final EffectId effect;
  final JobId jobId;
  final RenderSpec spec;
  @override
  bool operator ==(Object other) =>
      other is RenderRequest &&
      other.effect == effect &&
      other.jobId == jobId &&
      other.spec == spec;
  @override
  int get hashCode => Object.hash(effect, jobId, spec);
  @override
  String toString() => 'RenderRequest#$effect($jobId)';
}

/// The UI isolate's answer to the [RenderRequest] naming [effect].
sealed class RenderResult {
  const RenderResult(this.effect, this.jobId);
  final EffectId effect;
  final JobId jobId;
}

/// The render drew and answered [outcome].
final class RenderAnswered extends RenderResult {
  const RenderAnswered(super.effect, super.jobId, this.outcome);
  final RenderOutcome outcome;
  @override
  bool operator ==(Object other) =>
      other is RenderAnswered &&
      other.effect == effect &&
      other.jobId == jobId &&
      other.outcome == outcome;
  @override
  int get hashCode => Object.hash(effect, jobId, outcome);
  @override
  String toString() => 'RenderAnswered#$effect($jobId)';
}

/// The render's WebView went away before it answered.
final class RenderGone extends RenderResult {
  const RenderGone(super.effect, super.jobId);
  @override
  bool operator ==(Object other) =>
      other is RenderGone && other.effect == effect && other.jobId == jobId;
  @override
  int get hashCode => Object.hash(71, effect, jobId);
  @override
  String toString() => 'RenderGone#$effect($jobId)';
}

/// What the worker sends the UI isolate.
sealed class ToUi {
  const ToUi();
}

/// One of the worker's effects the UI carries out: `PublishState`,
/// `Notice` or `KeepAlive`. Any effect encodes.
final class UiEffect extends ToUi {
  const UiEffect(this.effect);
  final Effect effect;
  @override
  bool operator ==(Object other) => other is UiEffect && other.effect == effect;
  @override
  int get hashCode => Object.hash(81, effect);
  @override
  String toString() => 'UiEffect($effect)';
}

/// A render asked of the UI isolate.
final class UiRender extends ToUi {
  const UiRender(this.request);
  final RenderRequest request;
  @override
  bool operator ==(Object other) =>
      other is UiRender && other.request == request;
  @override
  int get hashCode => Object.hash(82, request);
  @override
  String toString() => 'UiRender($request)';
}

/// What the UI isolate sends the worker.
sealed class ToWorker {
  const ToWorker();
}

/// A message for the worker's loop: an intent, `CanRender`, `Woke`,
/// `OutOfTime`. Any message encodes.
final class WorkerMsg extends ToWorker {
  const WorkerMsg(this.msg);
  final Msg msg;
  @override
  bool operator ==(Object other) => other is WorkerMsg && other.msg == msg;
  @override
  int get hashCode => Object.hash(91, msg);
  @override
  String toString() => 'WorkerMsg($msg)';
}

/// A render's answer, for the effect runner.
final class WorkerRender extends ToWorker {
  const WorkerRender(this.result);
  final RenderResult result;
  @override
  bool operator ==(Object other) =>
      other is WorkerRender && other.result == result;
  @override
  int get hashCode => Object.hash(92, result);
  @override
  String toString() => 'WorkerRender(${result.runtimeType})';
}

// ------------------------------------------------------------ decoding results

enum CodecErrorKind {
  /// Not an envelope, or one of another [codecVersion].
  version,

  /// A tag (of a message, effect, or nested variant) the codec does not know.
  unknownTag,

  /// A field missing, or of the wrong type or value.
  malformed,
}

/// Why a value did not decode: [path] names the field (`$.job.steps[2]`).
final class CodecError {
  const CodecError(this.kind, this.path, this.message);
  final CodecErrorKind kind;
  final String path;
  final String message;
  @override
  bool operator ==(Object other) =>
      other is CodecError &&
      other.kind == kind &&
      other.path == path &&
      other.message == message;
  @override
  int get hashCode => Object.hash(kind, path, message);
  @override
  String toString() => 'CodecError(${kind.name} at $path: $message)';
}

/// A decoder's answer.
sealed class Decoded<T> {
  const Decoded();

  /// The value, or null for a failure.
  T? get valueOrNull => switch (this) {
    DecodeOk(:final value) => value,
    DecodeFailure() => null,
  };
}

final class DecodeOk<T> extends Decoded<T> {
  const DecodeOk(this.value);
  final T value;
  @override
  String toString() => 'DecodeOk($value)';
}

final class DecodeFailure<T> extends Decoded<T> {
  const DecodeFailure(this.error);
  final CodecError error;
  @override
  String toString() => 'DecodeFailure($error)';
}

// ------------------------------------------------------------ the API

Map<String, Object?> encodeMsg(Msg m) => throw UnimplementedError();
Decoded<Msg> decodeMsg(Object? wire) => _unimplemented();

Map<String, Object?> encodeEffect(Effect e) => throw UnimplementedError();
Decoded<Effect> decodeEffect(Object? wire) => _unimplemented();

Map<String, Object?> encodeQueueState(QueueState s) =>
    throw UnimplementedError();
Decoded<QueueState> decodeQueueState(Object? wire) => _unimplemented();

Map<String, Object?> encodeRenderRequest(RenderRequest r) =>
    throw UnimplementedError();
Decoded<RenderRequest> decodeRenderRequest(Object? wire) => _unimplemented();

Map<String, Object?> encodeRenderResult(RenderResult r) =>
    throw UnimplementedError();
Decoded<RenderResult> decodeRenderResult(Object? wire) => _unimplemented();

Map<String, Object?> encodeToUi(ToUi m) => throw UnimplementedError();
Decoded<ToUi> decodeToUi(Object? wire) => _unimplemented();

Map<String, Object?> encodeToWorker(ToWorker m) => throw UnimplementedError();
Decoded<ToWorker> decodeToWorker(Object? wire) => _unimplemented();

Decoded<T> _unimplemented<T>() => const DecodeFailure(
  CodecError(CodecErrorKind.malformed, r'$', 'not implemented'),
);
