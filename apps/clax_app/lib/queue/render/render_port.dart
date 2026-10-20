/// The render boundary between the worker and the UI isolate (spec
/// 2026-10-19-in-process-core-design §7.3).
///
/// The worker holds a [RenderPort]: it [RenderPort.ask]s a [RenderRequest]
/// for each `Render` effect and hears [RenderReply]s, each of which is one
/// of the queue's messages ([RenderReply.toMsg]): a [RenderResult] is
/// `Rendered` or `RenderLost`, a [RenderNews] is `CanRender`. The UI
/// isolate holds the other end, a [RenderEndpoint], which `RenderService`
/// (lib/ui/render_service.dart) serves.
///
/// Neither end names an isolate. On the wire every message is a map of
/// strings, numbers, booleans, lists, maps and a `Uint8List`
/// ([encodeRender]), which a `SendPort` copies as it is; [WireRenderPort]
/// and [WireRenderEndpoint] put either end over any such wire, and
/// [inProcessRender] joins the two in one isolate.
///
/// The worker's clock alone decides that a render took too long (its
/// 60 s deadline is `update`'s, rule 8): the UI isolate never times a
/// request out, and answers late ones as if on time; `update` drops an
/// answer to a `Render` no longer in flight.
library;

import 'dart:async';
import 'dart:typed_data';

import '../effect.dart';
import '../model.dart';
import '../msg.dart';

/// Any message of the render boundary.
sealed class RenderMessage {
  const RenderMessage();
}

/// What the worker sends the UI isolate.
sealed class RenderAsk extends RenderMessage {
  const RenderAsk();
}

/// What the UI isolate sends the worker.
sealed class RenderReply extends RenderMessage {
  const RenderReply();

  /// The queue's message this reply is. The effect runner records a done
  /// [RenderResult] in the core (spec §8.2) before it hands this to
  /// `update`.
  Msg toMsg();
}

/// The migration a [RenderKind.migration] request runs: the bundle's
/// [file], from the installed version [from] to the bundle's [to].
final class MigrationSpec {
  const MigrationSpec({
    this.file = 'migrate.js',
    required this.from,
    required this.to,
  });
  final String file;
  final int from;
  final int to;
  @override
  bool operator ==(Object other) =>
      other is MigrationSpec &&
      other.file == file &&
      other.from == from &&
      other.to == to;
  @override
  int get hashCode => Object.hash(file, from, to);
}

/// Render the draft [draftId] at [revision] for the `render` call [callId]
/// of job [jobId]; [id] is the `Render` effect's, which the answer names.
///
/// A request for a call that the UI isolate is still rendering ends that
/// render (answered lost): the worker asks a call again only once it
/// counted the earlier ask lost.
final class RenderRequest extends RenderAsk {
  const RenderRequest({
    required this.id,
    required this.jobId,
    required this.callId,
    required this.draftId,
    required this.revision,
    this.kind = RenderKind.inspect,
    this.settleMs = defaultSettleMs,
    this.maxBytes = defaultMaxBytes,
    this.files = const [],
    this.migration,
  });

  /// The request for `Render` effect [e]. A [RenderKind.tests] request
  /// runs [files]; a [RenderKind.migration] one runs [migration].
  factory RenderRequest.of(
    Render e, {
    List<String> files = const [],
    MigrationSpec? migration,
  }) => RenderRequest(
    id: e.id,
    jobId: e.jobId,
    callId: e.callId,
    draftId: e.spec.draftId,
    revision: e.spec.revision,
    kind: e.spec.kind,
    files: files,
    migration: migration,
  );

  static const defaultSettleMs = 3000;
  static const defaultMaxBytes = 1 << 20;

  final EffectId id;
  final JobId jobId;
  final String callId;
  final String draftId;
  final int revision;
  final RenderKind kind;

  /// How long the page may take to settle before an inspection.
  final int settleMs;

  /// The most an inspection's screenshot may weigh.
  final int maxBytes;

  /// The draft's test files a tests run runs (`tests/<name>.test.js`).
  final List<String> files;
  final MigrationSpec? migration;

  @override
  bool operator ==(Object other) =>
      other is RenderRequest &&
      other.id == id &&
      other.jobId == jobId &&
      other.callId == callId &&
      other.draftId == draftId &&
      other.revision == revision &&
      other.kind == kind &&
      other.settleMs == settleMs &&
      other.maxBytes == maxBytes &&
      listEquals(other.files, files) &&
      other.migration == migration;
  @override
  int get hashCode => Object.hash(
    id,
    jobId,
    callId,
    draftId,
    revision,
    kind,
    settleMs,
    maxBytes,
    Object.hashAll(files),
    migration,
  );
  @override
  String toString() => 'RenderRequest#$id($jobId, $callId, ${kind.name})';
}

/// Stop every render of job [jobId]: the worker abandoned its work
/// (`StopWork`). Each is answered lost.
final class RenderCancel extends RenderAsk {
  const RenderCancel(this.jobId);
  final JobId jobId;
  @override
  bool operator ==(Object other) =>
      other is RenderCancel && other.jobId == jobId;
  @override
  int get hashCode => jobId.hashCode;
  @override
  String toString() => 'RenderCancel($jobId)';
}

/// The answer to request [id] of job [jobId]: what the render found
/// ([outcome], with the page's [png] for an inspection), or that it was
/// lost ([isLost]: no WebView could be made, or it was torn down, before
/// the page answered).
///
/// A render that happened is answered with an [outcome] even when it went
/// wrong: a page that does not load or answer has [RenderOutcome.clean]
/// false and its `data` is `{'error': message}`. Otherwise `data` is one of
/// `{'inspection': {...}}`, `{'tests': {...}}` or `{'migration': {...}}`.
final class RenderResult extends RenderReply {
  const RenderResult.done(
    this.id,
    this.jobId,
    RenderOutcome this.outcome, {
    this.png,
  });
  const RenderResult.lost(this.id, this.jobId) : outcome = null, png = null;

  final EffectId id;
  final JobId jobId;

  /// Null when the render was lost.
  final RenderOutcome? outcome;
  final Uint8List? png;

  bool get isLost => outcome == null;

  @override
  Msg toMsg() => switch (outcome) {
    final o? => Rendered(id, jobId, o),
    null => RenderLost(id, jobId),
  };

  @override
  bool operator ==(Object other) =>
      other is RenderResult &&
      other.id == id &&
      other.jobId == jobId &&
      other.outcome == outcome &&
      _bytesEqual(other.png, png);
  @override
  int get hashCode => Object.hash(id, jobId, outcome, png?.length);
  @override
  String toString() =>
      'RenderResult#$id($jobId, ${isLost ? 'lost' : 'clean: ${outcome!.clean}'})';
}

/// Whether the UI isolate can make a WebView now: false while its engine is
/// detached or the app is in the background. Sent when an endpoint is
/// served and on every change.
final class RenderNews extends RenderReply {
  const RenderNews({required this.canRender});
  final bool canRender;
  @override
  Msg toMsg() => CanRender(canRender);
  @override
  bool operator ==(Object other) =>
      other is RenderNews && other.canRender == canRender;
  @override
  int get hashCode => canRender.hashCode;
  @override
  String toString() => 'RenderNews($canRender)';
}

bool _bytesEqual(Uint8List? a, Uint8List? b) {
  if (a == null || b == null) return a == b;
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

// ------------------------------------------------------------ the wire

/// The key every render message's map carries, naming its type, so a wire
/// that carries other messages too can pick these out ([isRenderWire]).
const renderWireKey = 'render';

/// [m] as a map a `SendPort` sends as it is.
Map<String, Object?> encodeRender(RenderMessage m) => switch (m) {
  RenderRequest() => {
    renderWireKey: 'request',
    'id': m.id,
    'jobId': m.jobId,
    'callId': m.callId,
    'draftId': m.draftId,
    'revision': m.revision,
    'kind': m.kind.name,
    'settleMs': m.settleMs,
    'maxBytes': m.maxBytes,
    'files': [...m.files],
    if (m.migration case final mi?)
      'migration': {'file': mi.file, 'from': mi.from, 'to': mi.to},
  },
  RenderCancel() => {renderWireKey: 'cancel', 'jobId': m.jobId},
  RenderResult() => {
    renderWireKey: 'result',
    'id': m.id,
    'jobId': m.jobId,
    if (m.outcome case final o?) ...{
      'clean': o.clean,
      'data': _copyJson(o.data),
      if (m.png case final png?) 'png': Uint8List.fromList(png),
    },
  },
  RenderNews() => {renderWireKey: 'news', 'canRender': m.canRender},
};

/// Whether [wire] is a render message's map.
bool isRenderWire(Object? wire) => wire is Map && wire[renderWireKey] is String;

/// The render message [wire] encodes; null for any other value.
RenderMessage? decodeRender(Object? wire) {
  if (!isRenderWire(wire)) return null;
  final w = wire! as Map;
  return switch (w[renderWireKey]) {
    'request' => RenderRequest(
      id: w['id'] as int,
      jobId: w['jobId'] as String,
      callId: w['callId'] as String,
      draftId: w['draftId'] as String,
      revision: w['revision'] as int,
      kind: RenderKind.values.byName(w['kind'] as String),
      settleMs: w['settleMs'] as int,
      maxBytes: w['maxBytes'] as int,
      files: [for (final f in w['files'] as List) f as String],
      migration: switch (w['migration']) {
        final Map mi => MigrationSpec(
          file: mi['file'] as String,
          from: mi['from'] as int,
          to: mi['to'] as int,
        ),
        _ => null,
      },
    ),
    'cancel' => RenderCancel(w['jobId'] as String),
    'result' when w.containsKey('clean') => RenderResult.done(
      w['id'] as int,
      w['jobId'] as String,
      RenderOutcome(clean: w['clean'] as bool, data: w['data']),
      png: w['png'] as Uint8List?,
    ),
    'result' => RenderResult.lost(w['id'] as int, w['jobId'] as String),
    'news' => RenderNews(canRender: w['canRender'] as bool),
    _ => null,
  };
}

/// A copy of JSON value [v], so no map or list is shared across the wire.
Object? _copyJson(Object? v) => switch (v) {
  final Map m => {
    for (final e in m.entries) e.key as String: _copyJson(e.value),
  },
  final List l => [for (final x in l) _copyJson(x)],
  _ => v,
};

// ------------------------------------------------------------ the two ends

/// The worker's end of the render boundary: what the effect runner calls.
abstract interface class RenderPort {
  /// Asks the UI isolate to render; its [RenderResult] comes on [replies].
  void ask(RenderRequest request);

  /// Ends every render of job [jobId] (on `StopWork`).
  void cancel(JobId jobId);

  /// The UI isolate's replies, in the order sent: each is a message for
  /// `update` ([RenderReply.toMsg]).
  Stream<RenderReply> get replies;
}

/// The UI isolate's end of the render boundary, which `RenderService`
/// serves.
abstract interface class RenderEndpoint {
  /// The worker's requests and cancels, in the order sent.
  Stream<RenderAsk> get asks;

  void reply(RenderReply reply);
}

/// The worker's end over a wire: [post] sends a map to the UI isolate (a
/// `SendPort`'s `send`), [incoming] is what the UI isolate sent, of which
/// only render messages are taken.
final class WireRenderPort implements RenderPort {
  WireRenderPort(this._post, Stream<Object?> incoming)
    : replies = incoming
          .map(decodeRender)
          .where((m) => m is RenderReply)
          .cast<RenderReply>();

  final void Function(Object? message) _post;

  @override
  final Stream<RenderReply> replies;

  @override
  void ask(RenderRequest request) => _post(encodeRender(request));

  @override
  void cancel(JobId jobId) => _post(encodeRender(RenderCancel(jobId)));
}

/// The UI isolate's end over a wire: [post] sends a map to the worker,
/// [incoming] is what the worker sent, of which only render messages are
/// taken.
final class WireRenderEndpoint implements RenderEndpoint {
  WireRenderEndpoint(this._post, Stream<Object?> incoming)
    : asks = incoming
          .map(decodeRender)
          .where((m) => m is RenderAsk)
          .cast<RenderAsk>();

  final void Function(Object? message) _post;

  @override
  final Stream<RenderAsk> asks;

  @override
  void reply(RenderReply reply) => _post(encodeRender(reply));
}

/// Both ends joined in this isolate, each message going as a map through
/// [encodeRender] and [decodeRender], as across isolates, and delivered
/// asynchronously in order.
(RenderPort, RenderEndpoint) inProcessRender() {
  final toUi = StreamController<Object?>.broadcast();
  final toWorker = StreamController<Object?>.broadcast();
  return (
    WireRenderPort(toUi.add, toWorker.stream),
    WireRenderEndpoint(toWorker.add, toUi.stream),
  );
}
