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

/// What the worker asks the UI isolate to render: [spec], for the `render`
/// call [callId] of job [jobId], as the `Render` effect [effect]. Its
/// answer is the [RenderResult] naming the same [effect].
///
/// A request for a call the UI isolate is still rendering ends that render
/// (answered gone): the worker asks a call again only once it counted the
/// earlier ask lost. The UI isolate keeps no time of its own: the worker's
/// deadline decides that a render took too long.
final class RenderRequest {
  const RenderRequest(
    this.effect,
    this.jobId,
    this.callId,
    this.spec, {
    this.settleMs = defaultSettleMs,
    this.maxBytes = defaultMaxBytes,
  });

  /// The request for `Render` effect [e].
  factory RenderRequest.of(Render e) =>
      RenderRequest(e.id, e.jobId, e.callId, e.spec);

  static const defaultSettleMs = 3000;
  static const defaultMaxBytes = 1 << 20;

  final EffectId effect;
  final JobId jobId;
  final String callId;
  final RenderSpec spec;

  /// How long the page may take to settle before an inspection.
  final int settleMs;

  /// The most an inspection's screenshot may weigh.
  final int maxBytes;

  @override
  bool operator ==(Object other) =>
      other is RenderRequest &&
      other.effect == effect &&
      other.jobId == jobId &&
      other.callId == callId &&
      other.spec == spec &&
      other.settleMs == settleMs &&
      other.maxBytes == maxBytes;
  @override
  int get hashCode =>
      Object.hash(effect, jobId, callId, spec, settleMs, maxBytes);
  @override
  String toString() => 'RenderRequest#$effect($jobId, $callId)';
}

/// The UI isolate's answer to the [RenderRequest] naming [effect].
sealed class RenderResult {
  const RenderResult(this.effect, this.jobId);
  final EffectId effect;
  final JobId jobId;

  /// The queue's message this answer is.
  Msg toMsg();
}

/// The render drew and answered [outcome], even one that went wrong (a
/// page that did not load answers `{'error': message}`, not clean).
final class RenderAnswered extends RenderResult {
  const RenderAnswered(super.effect, super.jobId, this.outcome);
  final RenderOutcome outcome;
  @override
  Msg toMsg() => Rendered(effect, jobId, outcome);
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

/// The render was lost before it answered: no WebView could be made, it
/// was torn down, the call was asked again, or the job's renders were
/// cancelled.
final class RenderGone extends RenderResult {
  const RenderGone(super.effect, super.jobId);
  @override
  Msg toMsg() => RenderLost(effect, jobId);
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

/// What the UI isolate's render endpoint takes: a render, or an end to a
/// job's renders.
sealed class UiRenderAsk extends ToUi {
  const UiRenderAsk();
}

/// A render asked of the UI isolate.
final class UiRender extends UiRenderAsk {
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

/// Ends every render of job [jobId] (the worker stopped its work); each is
/// answered gone.
final class UiRenderCancel extends UiRenderAsk {
  const UiRenderCancel(this.jobId);
  final JobId jobId;
  @override
  bool operator ==(Object other) =>
      other is UiRenderCancel && other.jobId == jobId;
  @override
  int get hashCode => Object.hash(83, jobId);
  @override
  String toString() => 'UiRenderCancel($jobId)';
}

/// What the UI isolate sends the worker.
sealed class ToWorker {
  const ToWorker();
}

/// A message for the worker's loop: an intent, `CanRender`, `Woke`,
/// `OutOfTime`. Any message encodes; the worker takes only those
/// ([fromUi]).
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

/// Whether the UI isolate may send [m] to the worker's loop: an intent,
/// `CanRender`, `Woke` or `OutOfTime`. Every other message is the worker's
/// own, and the worker refuses it from the UI.
bool fromUi(Msg m) =>
    m is Intent || m is CanRender || m is Woke || m is OutOfTime;

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

/// A message, as an envelope.
Map<String, Object?> encodeMsg(Msg m) => _top(_msg(m));
Decoded<Msg> decodeMsg(Object? wire) => _decodeTop(wire, _msgOf);

/// An effect, as an envelope.
Map<String, Object?> encodeEffect(Effect e) => _top(_effect(e));
Decoded<Effect> decodeEffect(Object? wire) => _decodeTop(wire, _effectOf);

Map<String, Object?> encodeQueueState(QueueState s) => _top(_state(s));
Decoded<QueueState> decodeQueueState(Object? wire) =>
    _decodeTop(wire, _stateOf);

Map<String, Object?> encodeRenderRequest(RenderRequest r) =>
    _top(_renderRequest(r));
Decoded<RenderRequest> decodeRenderRequest(Object? wire) =>
    _decodeTop(wire, _renderRequestOf);

Map<String, Object?> encodeRenderResult(RenderResult r) =>
    _top(_renderResult(r));
Decoded<RenderResult> decodeRenderResult(Object? wire) =>
    _decodeTop(wire, _renderResultOf);

/// What the worker posts to the UI isolate's port.
Map<String, Object?> encodeToUi(ToUi m) => _top(switch (m) {
  UiEffect(:final effect) => {'t': 'effect', 'effect': _effect(effect)},
  UiRender(:final request) => {
    't': 'render',
    'request': _renderRequest(request),
  },
  UiRenderCancel(:final jobId) => {'t': 'renderCancel', 'jobId': jobId},
});
Decoded<ToUi> decodeToUi(Object? wire) => _decodeTop(
  wire,
  (r) => r.variant<ToUi>({
    'effect': (r) => UiEffect(r.obj('effect', _effectOf)),
    'render': (r) => UiRender(r.obj('request', _renderRequestOf)),
    'renderCancel': (r) => UiRenderCancel(r.str('jobId')),
  }),
);

/// What the UI isolate posts to the worker's port.
Map<String, Object?> encodeToWorker(ToWorker m) => _top(switch (m) {
  WorkerMsg(:final msg) => {'t': 'msg', 'msg': _msg(msg)},
  WorkerRender(:final result) => {
    't': 'render',
    'result': _renderResult(result),
  },
});
Decoded<ToWorker> decodeToWorker(Object? wire) => _decodeTop(
  wire,
  (r) => r.variant<ToWorker>({
    'msg': (r) => WorkerMsg(r.obj('msg', _msgOf)),
    'render': (r) => WorkerRender(r.obj('result', _renderResultOf)),
  }),
);

/// The tags [encodeMsg] writes, one per message type.
Iterable<String> get msgTags => _msgDecoders.keys;

/// The tags [encodeEffect] writes, one per effect type.
Iterable<String> get effectTags => _effectDecoders.keys;

// ------------------------------------------------------------ envelopes

Map<String, Object?> _top(Map<String, Object?> body) => {
  'v': codecVersion,
  ...body,
};

Decoded<T> _decodeTop<T>(Object? wire, T Function(_R) body) {
  try {
    if (wire is! Map) {
      throw _Fail(
        CodecErrorKind.version,
        r'$',
        'not an envelope: ${_kindOf(wire)}',
      );
    }
    final r = _R.of(wire, r'$');
    final v = r.m['v'];
    if (v is! int || v != codecVersion) {
      throw _Fail(
        CodecErrorKind.version,
        r'$.v',
        'version ${v ?? 'missing'}, not $codecVersion',
      );
    }
    return DecodeOk(body(r));
  } on _Fail catch (f) {
    return DecodeFailure(CodecError(f.kind, f.path, f.message));
  } catch (e) {
    // A defect of the input the reads above do not name (a data value
    // nested deeper than the stack, say) is still the input's.
    return DecodeFailure(
      CodecError(CodecErrorKind.malformed, r'$', e.runtimeType.toString()),
    );
  }
}

final class _Fail {
  const _Fail(this.kind, this.path, this.message);
  final CodecErrorKind kind;
  final String path;
  final String message;
}

String _kindOf(Object? v) => switch (v) {
  null => 'null',
  String() => 'a string',
  bool() => 'a boolean',
  num() => 'a number',
  List() => 'a list',
  Map() => 'a map',
  _ => v.runtimeType.toString(),
};

/// A map being decoded at [path]. Each read fails with the field's path.
final class _R {
  _R._(this.m, this.path);
  final Map<Object?, Object?> m;
  final String path;

  static _R of(Object? v, String path) {
    if (v is! Map) {
      throw _Fail(CodecErrorKind.malformed, path, 'not a map: ${_kindOf(v)}');
    }
    for (final k in v.keys) {
      if (k is! String) {
        throw _Fail(CodecErrorKind.malformed, path, 'a key not a string: $k');
      }
    }
    return _R._(v, path);
  }

  String at(String k) => '$path.$k';

  Never fail(String k, String message) =>
      throw _Fail(CodecErrorKind.malformed, at(k), message);

  Object? raw(String k) {
    if (!m.containsKey(k)) fail(k, 'missing');
    return m[k];
  }

  T _typed<T>(String k, String what) {
    final v = raw(k);
    if (v is! T) fail(k, 'not $what: ${_kindOf(v)}');
    return v;
  }

  String str(String k) => _typed<String>(k, 'a string');
  String? strOrNull(String k) => _typed<String?>(k, 'a string or null');
  int integer(String k) => _typed<int>(k, 'an integer');
  bool boolean(String k) => _typed<bool>(k, 'a boolean');

  T enumOf<T extends Enum>(String k, List<T> values) {
    final name = str(k);
    for (final v in values) {
      if (v.name == name) return v;
    }
    fail(k, 'not one of ${values.map((v) => v.name).join(', ')}: $name');
  }

  T? enumOrNull<T extends Enum>(String k, List<T> values) =>
      raw(k) == null ? null : enumOf(k, values);

  T obj<T>(String k, T Function(_R) dec) => dec(_R.of(raw(k), at(k)));

  T? objOrNull<T>(String k, T Function(_R) dec) =>
      raw(k) == null ? null : obj(k, dec);

  List<T> list<T>(String k, T Function(Object? v, String path) dec) {
    final v = _typed<List<Object?>>(k, 'a list');
    return [for (var i = 0; i < v.length; i++) dec(v[i], '${at(k)}[$i]')];
  }

  List<T> objs<T>(String k, T Function(_R) dec) =>
      list(k, (v, p) => dec(_R.of(v, p)));

  /// A map with string keys, its values decoded by [dec], in its order.
  Map<String, T> dict<T>(String k, T Function(Object? v, String path) dec) {
    final d = _R.of(raw(k), at(k));
    return {
      for (final e in d.m.entries)
        e.key! as String: dec(e.value, d.at(e.key! as String)),
    };
  }

  /// A JSON value, copied.
  Object? json(String k) => _jsonOf(raw(k), at(k));

  DateTime time(String k) => obj(k, (r) {
    final us = r.integer('us');
    final utc = r.boolean('utc');
    try {
      return DateTime.fromMicrosecondsSinceEpoch(us, isUtc: utc);
    } on ArgumentError {
      throw _Fail(CodecErrorKind.malformed, at(k), 'out of range: $us');
    }
  });

  DateTime? timeOrNull(String k) => raw(k) == null ? null : time(k);

  /// The variant field `t` names, decoded by its entry in [decoders].
  T variant<T>(Map<String, T Function(_R)> decoders) {
    final tag = str('t');
    final dec = decoders[tag];
    if (dec == null) {
      throw _Fail(CodecErrorKind.unknownTag, at('t'), 'unknown tag: $tag');
    }
    return dec(this);
  }
}

String _str(Object? v, String path) => v is String
    ? v
    : throw _Fail(
        CodecErrorKind.malformed,
        path,
        'not a string: ${_kindOf(v)}',
      );

/// [v] as a JSON value, copied into plain maps and lists; a value JSON
/// cannot carry (an object, a map with a key not a string, a number not
/// finite) fails.
Object? _jsonOf(Object? v, String path) => switch (v) {
  null || String() || bool() || int() => v,
  double() when v.isFinite => v,
  List() => [for (var i = 0; i < v.length; i++) _jsonOf(v[i], '$path[$i]')],
  Map() => {
    for (final e in _R.of(v, path).m.entries)
      e.key! as String: _jsonOf(e.value, '$path.${e.key}'),
  },
  _ => throw _Fail(
    CodecErrorKind.malformed,
    path,
    'not a JSON value: ${_kindOf(v)}',
  ),
};

/// A JSON value, copied into plain maps and lists. A value that is not
/// JSON is passed on as it is, for the decoder to refuse: encoding never
/// fails.
Object? _json(Object? v) => switch (v) {
  List() => [for (final x in v) _json(x)],
  Map() => {for (final e in v.entries) e.key: _json(e.value)},
  _ => v,
};

/// A time to the microsecond, in its zone (UTC or local).
Map<String, Object?> _time(DateTime t) => {
  'us': t.microsecondsSinceEpoch,
  'utc': t.isUtc,
};

// ------------------------------------------------------------ messages

Map<String, Object?> _msg(Msg m) => switch (m) {
  Make(:final id, :final request, :final title) => {
    't': 'make',
    'id': id,
    'request': request,
    'title': title,
  },
  Change(
    :final id,
    :final artifactId,
    :final base,
    :final threads,
    :final request,
    :final title,
  ) =>
    {
      't': 'change',
      'id': id,
      'artifactId': artifactId,
      'base': base,
      'threads': {...threads},
      'request': request,
      'title': title,
    },
  Cancel(:final jobId) => {'t': 'cancel', 'jobId': jobId},
  Retry(:final jobId) => {'t': 'retry', 'jobId': jobId},
  KeepGoing(:final jobId) => {'t': 'keepGoing', 'jobId': jobId},
  Discard(:final jobId) => {'t': 'discard', 'jobId': jobId},
  CoreOpened() => {'t': 'coreOpened'},
  OwnerClaimed(:final effect, :final token) => {
    't': 'ownerClaimed',
    'effect': effect,
    'token': token,
  },
  OwnerHeld(:final effect) => {'t': 'ownerHeld', 'effect': effect},
  OwnerDraining(:final effect) => {'t': 'ownerDraining', 'effect': effect},
  OwnerChanged(:final client, :final held) => {
    't': 'ownerChanged',
    'client': client,
    'held': held,
  },
  JobsLoaded(:final jobs) => {
    't': 'jobsLoaded',
    'jobs': [for (final j in jobs) _job(j)],
  },
  JobNoticed(:final id, :final client, :final state, :final step, :final rev) =>
    {
      't': 'jobNoticed',
      'id': id,
      'client': client,
      'state': state.name,
      'step': step == null ? null : _stepKey(step),
      'rev': rev,
    },
  JobRead(:final effect, :final jobId, :final job) => {
    't': 'jobRead',
    'effect': effect,
    'jobId': jobId,
    'job': job == null ? null : _job(job),
  },
  Committed(:final effect, :final job, :final step, :final replayed) => {
    't': 'committed',
    'effect': effect,
    'job': _job(job),
    'step': step == null ? null : _stepResult(step),
    'replayed': replayed,
  },
  Refused(:final effect, :final jobId, :final reason, :final job) => {
    't': 'refused',
    'effect': effect,
    'jobId': jobId,
    'reason': _refusal(reason),
    'job': job == null ? null : _job(job),
  },
  TurnDone(:final effect, :final jobId, :final outcome) => {
    't': 'turnDone',
    'effect': effect,
    'jobId': jobId,
    'outcome': _turnOutcome(outcome),
  },
  TurnFailed(:final effect, :final jobId, :final failure, :final detail) => {
    't': 'turnFailed',
    'effect': effect,
    'jobId': jobId,
    'failure': failure.name,
    'detail': detail,
  },
  ToolDone(:final effect, :final jobId, :final callId, :final result) => {
    't': 'toolDone',
    'effect': effect,
    'jobId': jobId,
    'callId': callId,
    'result': _toolResult(result),
  },
  Rendered(:final effect, :final jobId, :final outcome) => {
    't': 'rendered',
    'effect': effect,
    'jobId': jobId,
    'outcome': _renderOutcome(outcome),
  },
  RenderLost(:final effect, :final jobId) => {
    't': 'renderLost',
    'effect': effect,
    'jobId': jobId,
  },
  CanRender(:final can) => {'t': 'canRender', 'can': can},
  ClaudeReachable(:final reachable) => {
    't': 'claudeReachable',
    'reachable': reachable,
  },
  Tick(:final now) => {'t': 'tick', 'now': _time(now)},
  Woke(:final foreground) => {'t': 'woke', 'foreground': foreground},
  OutOfTime() => {'t': 'outOfTime'},
};

Msg _msgOf(_R r) => r.variant(_msgDecoders);

final Map<String, Msg Function(_R)> _msgDecoders = {
  'make': (r) =>
      Make(r.str('id'), r.str('request'), title: r.strOrNull('title')),
  'change': (r) => Change(
    r.str('id'),
    r.str('artifactId'),
    base: r.integer('base'),
    threads: r.dict('threads', _str),
    request: r.str('request'),
    title: r.strOrNull('title'),
  ),
  'cancel': (r) => Cancel(r.str('jobId')),
  'retry': (r) => Retry(r.str('jobId')),
  'keepGoing': (r) => KeepGoing(r.str('jobId')),
  'discard': (r) => Discard(r.str('jobId')),
  'coreOpened': (_) => const CoreOpened(),
  'ownerClaimed': (r) => OwnerClaimed(r.integer('effect'), r.str('token')),
  'ownerHeld': (r) => OwnerHeld(r.integer('effect')),
  'ownerDraining': (r) => OwnerDraining(r.integer('effect')),
  'ownerChanged': (r) =>
      OwnerChanged(client: r.str('client'), held: r.boolean('held')),
  'jobsLoaded': (r) => JobsLoaded(r.objs('jobs', _jobOf)),
  'jobNoticed': (r) => JobNoticed(
    r.str('id'),
    client: r.str('client'),
    state: r.enumOf('state', JobState.values),
    step: r.objOrNull('step', _stepKeyOf),
    rev: r.integer('rev'),
  ),
  'jobRead': (r) =>
      JobRead(r.integer('effect'), r.str('jobId'), r.objOrNull('job', _jobOf)),
  'committed': (r) => Committed(
    r.integer('effect'),
    r.obj('job', _jobOf),
    step: r.objOrNull('step', _stepResultOf),
    replayed: r.boolean('replayed'),
  ),
  'refused': (r) => Refused(
    r.integer('effect'),
    r.str('jobId'),
    r.obj('reason', _refusalOf),
    job: r.objOrNull('job', _jobOf),
  ),
  'turnDone': (r) => TurnDone(
    r.integer('effect'),
    r.str('jobId'),
    r.obj('outcome', _turnOutcomeOf),
  ),
  'turnFailed': (r) => TurnFailed(
    r.integer('effect'),
    r.str('jobId'),
    r.enumOf('failure', TurnFailure.values),
    detail: r.strOrNull('detail'),
  ),
  'toolDone': (r) => ToolDone(
    r.integer('effect'),
    r.str('jobId'),
    r.str('callId'),
    r.obj('result', _toolResultOf),
  ),
  'rendered': (r) => Rendered(
    r.integer('effect'),
    r.str('jobId'),
    r.obj('outcome', _renderOutcomeOf),
  ),
  'renderLost': (r) => RenderLost(r.integer('effect'), r.str('jobId')),
  'canRender': (r) => CanRender(r.boolean('can')),
  'claudeReachable': (r) => ClaudeReachable(r.boolean('reachable')),
  'tick': (r) => Tick(r.time('now')),
  'woke': (r) => Woke(foreground: r.boolean('foreground')),
  'outOfTime': (_) => const OutOfTime(),
};

Map<String, Object?> _refusal(Refusal r) => switch (r) {
  VersionMoved(:final current) => {'t': 'versionMoved', 'current': current},
  DocMoved(:final path) => {'t': 'docMoved', 'path': path},
  NotRenderedClean() => {'t': 'notRenderedClean'},
  JobMoved() => {'t': 'jobMoved'},
  NotOwner() => {'t': 'notOwner'},
  Invalid(:final code, :final message) => {
    't': 'invalid',
    'code': code,
    'message': message,
  },
};

Refusal _refusalOf(_R r) => r.variant<Refusal>({
  'versionMoved': (r) => VersionMoved(r.integer('current')),
  'docMoved': (r) => DocMoved(r.str('path')),
  'notRenderedClean': (_) => const NotRenderedClean(),
  'jobMoved': (_) => const JobMoved(),
  'notOwner': (_) => const NotOwner(),
  'invalid': (r) => Invalid(r.str('code'), r.strOrNull('message')),
});

// ------------------------------------------------------------ effects

Map<String, Object?> _effect(Effect e) => switch (e) {
  CreateJob(
    :final id,
    :final jobId,
    :final kind,
    :final input,
    :final artifactId,
    :final title,
    :final plan,
  ) =>
    {
      't': 'createJob',
      'id': id,
      'jobId': jobId,
      'kind': kind.name,
      'input': _jobInput(input),
      'artifactId': artifactId,
      'title': title,
      'plan': plan == null ? null : _plan(plan),
    },
  MoveJob(
    :final id,
    :final jobId,
    :final ifRev,
    :final to,
    :final error,
    :final carry,
    :final plan,
  ) =>
    {
      't': 'moveJob',
      'id': id,
      'jobId': jobId,
      'ifRev': ifRev,
      'to': to.name,
      'error': _field(error, _jobError),
      'carry': _field(carry, _carry),
      'plan': _field(plan, _plan),
    },
  CommitStep(:final id, :final jobId, :final turn, :final notes, :final base) =>
    {
      't': 'commitStep',
      'id': id,
      'jobId': jobId,
      'turn': turn,
      'notes': _notes(notes),
      'base': base,
    },
  Ask(:final id, :final jobId, :final turn, :final callId, :final question) => {
    't': 'ask',
    'id': id,
    'jobId': jobId,
    'turn': turn,
    'callId': callId,
    'question': _question(question),
  },
  Publish(
    :final id,
    :final jobId,
    :final turn,
    :final callId,
    :final publish,
    :final done,
  ) =>
    {
      't': 'publish',
      'id': id,
      'jobId': jobId,
      'turn': turn,
      'callId': callId,
      'publish': _publishPlan(publish),
      'done': done,
    },
  Reply(:final id, :final jobId, :final turn, :final callId, :final reply) => {
    't': 'reply',
    'id': id,
    'jobId': jobId,
    'turn': turn,
    'callId': callId,
    'reply': _replyPlan(reply),
  },
  StartTurn(:final id, :final jobId, :final input) => {
    't': 'startTurn',
    'id': id,
    'jobId': jobId,
    'input': _turnInput(input),
  },
  RunTool(:final id, :final jobId, :final turn, :final call) => {
    't': 'runTool',
    'id': id,
    'jobId': jobId,
    'turn': turn,
    'call': _otherCall(call),
  },
  Render(
    :final id,
    :final jobId,
    :final turn,
    :final callId,
    :final spec,
    :final last,
    :final deadline,
  ) =>
    {
      't': 'render',
      'id': id,
      'jobId': jobId,
      'turn': turn,
      'callId': callId,
      'spec': _renderSpec(spec),
      'last': last,
      'deadline': deadline == null ? null : _time(deadline),
    },
  ReadJob(:final id, :final jobId) => {
    't': 'readJob',
    'id': id,
    'jobId': jobId,
  },
  StopWork(:final id, :final jobId) => {
    't': 'stopWork',
    'id': id,
    'jobId': jobId,
  },
  ClaimOwner(:final id) => {'t': 'claimOwner', 'id': id},
  LoadJobs(:final id) => {'t': 'loadJobs', 'id': id},
  ProbeClaude(:final id) => {'t': 'probeClaude', 'id': id},
  Notice(
    :final id,
    :final kind,
    :final jobId,
    :final title,
    :final artifactId,
  ) =>
    {
      't': 'notice',
      'id': id,
      'kind': kind.name,
      'jobId': jobId,
      'title': title,
      'artifactId': artifactId,
    },
  KeepAlive(:final id, :final on) => {'t': 'keepAlive', 'id': id, 'on': on},
  PublishState(:final id, :final state) => {
    't': 'publishState',
    'id': id,
    'state': _state(state),
  },
};

Effect _effectOf(_R r) => r.variant(_effectDecoders);

final Map<String, Effect Function(_R)> _effectDecoders = {
  'createJob': (r) => CreateJob(
    r.integer('id'),
    r.str('jobId'),
    kind: r.enumOf('kind', JobKind.values),
    input: r.obj('input', _jobInputOf),
    artifactId: r.strOrNull('artifactId'),
    title: r.strOrNull('title'),
    plan: r.objOrNull('plan', _planOf),
  ),
  'moveJob': (r) => MoveJob(
    r.integer('id'),
    r.str('jobId'),
    ifRev: r.integer('ifRev'),
    to: r.enumOf('to', JobState.values),
    error: r.obj('error', (f) => _fieldOf(f, _jobErrorOf)),
    carry: r.obj('carry', (f) => _fieldOf(f, _carryOf)),
    plan: r.obj('plan', (f) => _fieldOf(f, _planOf)),
  ),
  'commitStep': (r) => CommitStep(
    r.integer('id'),
    r.str('jobId'),
    r.integer('turn'),
    r.obj('notes', _notesOf),
    base: r.integer('base'),
  ),
  'ask': (r) => Ask(
    r.integer('id'),
    r.str('jobId'),
    r.integer('turn'),
    r.str('callId'),
    r.obj('question', _questionOf),
  ),
  'publish': (r) => Publish(
    r.integer('id'),
    r.str('jobId'),
    r.integer('turn'),
    r.str('callId'),
    r.obj('publish', _publishPlanOf),
    done: r.boolean('done'),
  ),
  'reply': (r) => Reply(
    r.integer('id'),
    r.str('jobId'),
    r.integer('turn'),
    r.str('callId'),
    r.obj('reply', _replyPlanOf),
  ),
  'startTurn': (r) =>
      StartTurn(r.integer('id'), r.str('jobId'), r.obj('input', _turnInputOf)),
  'runTool': (r) => RunTool(
    r.integer('id'),
    r.str('jobId'),
    r.integer('turn'),
    r.obj('call', _otherCallOf),
  ),
  'render': (r) => Render(
    r.integer('id'),
    r.str('jobId'),
    r.integer('turn'),
    r.str('callId'),
    r.obj('spec', _renderSpecOf),
    last: r.boolean('last'),
    deadline: r.timeOrNull('deadline'),
  ),
  'readJob': (r) => ReadJob(r.integer('id'), r.str('jobId')),
  'stopWork': (r) => StopWork(r.integer('id'), r.str('jobId')),
  'claimOwner': (r) => ClaimOwner(r.integer('id')),
  'loadJobs': (r) => LoadJobs(r.integer('id')),
  'probeClaude': (r) => ProbeClaude(r.integer('id')),
  'notice': (r) => Notice(
    r.integer('id'),
    r.enumOf('kind', NoticeKind.values),
    r.str('jobId'),
    r.str('title'),
    artifactId: r.strOrNull('artifactId'),
  ),
  'keepAlive': (r) => KeepAlive(r.integer('id'), r.boolean('on')),
  'publishState': (r) =>
      PublishState(r.integer('id'), r.obj('state', _stateOf)),
};

Map<String, Object?> _field<T>(
  Field<T> f,
  Map<String, Object?> Function(T) enc,
) => switch (f) {
  Keep() => {'t': 'keep'},
  SetTo(:final value) => {'t': 'set', 'value': enc(value)},
  Clear() => {'t': 'clear'},
};

Field<T> _fieldOf<T>(_R r, T Function(_R) dec) => r.variant<Field<T>>({
  'keep': (_) => Keep<T>(),
  'set': (r) => SetTo<T>(r.obj('value', dec)),
  'clear': (_) => Clear<T>(),
});

// ------------------------------------------------------------ the UI's state

Map<String, Object?> _state(QueueState s) => {
  'jobs': [for (final j in s.jobs) _view(j)],
  'loaded': s.loaded,
};

QueueState _stateOf(_R r) =>
    QueueState(r.objs('jobs', _viewOf), loaded: r.boolean('loaded'));

Map<String, Object?> _view(JobView v) => {
  'id': v.id,
  'kind': v.kind.name,
  'state': v.state.name,
  'waitingFor': v.waitingFor?.name,
  'step': v.step == null ? null : _stepKey(v.step!),
  'title': v.title,
  'artifactId': v.artifactId,
  'error': v.error,
  'offers': [for (final o in v.offers) o.name],
};

JobView _viewOf(_R r) {
  final offers = <Offer>{};
  final names = r.list('offers', _str);
  for (var i = 0; i < names.length; i++) {
    final at = '${r.at('offers')}[$i]';
    final o = Offer.values.where((o) => o.name == names[i]).firstOrNull;
    if (o == null) {
      throw _Fail(CodecErrorKind.malformed, at, 'not an offer: ${names[i]}');
    }
    if (!offers.add(o)) {
      throw _Fail(CodecErrorKind.malformed, at, 'named twice: ${names[i]}');
    }
  }
  return JobView(
    id: r.str('id'),
    kind: r.enumOf('kind', JobKind.values),
    state: r.enumOf('state', JobState.values),
    waitingFor: r.enumOrNull('waitingFor', WaitingFor.values),
    step: r.objOrNull('step', _stepKeyOf),
    title: r.strOrNull('title'),
    artifactId: r.strOrNull('artifactId'),
    error: r.strOrNull('error'),
    offers: offers,
  );
}

// ------------------------------------------------------------ rendering

Map<String, Object?> _renderRequest(RenderRequest r) => {
  'effect': r.effect,
  'jobId': r.jobId,
  'callId': r.callId,
  'spec': _renderSpec(r.spec),
  'settleMs': r.settleMs,
  'maxBytes': r.maxBytes,
};

RenderRequest _renderRequestOf(_R r) => RenderRequest(
  r.integer('effect'),
  r.str('jobId'),
  r.str('callId'),
  r.obj('spec', _renderSpecOf),
  settleMs: r.integer('settleMs'),
  maxBytes: r.integer('maxBytes'),
);

Map<String, Object?> _renderResult(RenderResult r) => switch (r) {
  RenderAnswered(:final effect, :final jobId, :final outcome) => {
    't': 'answered',
    'effect': effect,
    'jobId': jobId,
    'outcome': _renderOutcome(outcome),
  },
  RenderGone(:final effect, :final jobId) => {
    't': 'gone',
    'effect': effect,
    'jobId': jobId,
  },
};

RenderResult _renderResultOf(_R r) => r.variant<RenderResult>({
  'answered': (r) => RenderAnswered(
    r.integer('effect'),
    r.str('jobId'),
    r.obj('outcome', _renderOutcomeOf),
  ),
  'gone': (r) => RenderGone(r.integer('effect'), r.str('jobId')),
});

Map<String, Object?> _renderSpec(RenderSpec s) => {
  'draftId': s.draftId,
  'revision': s.revision,
  'kind': s.kind.name,
  'digest': s.digest,
  'files': [...s.files],
  'migration': switch (s.migration) {
    final m? => {'file': m.file, 'from': m.from, 'to': m.to},
    null => null,
  },
};

RenderSpec _renderSpecOf(_R r) => RenderSpec(
  draftId: r.str('draftId'),
  revision: r.integer('revision'),
  kind: r.enumOf('kind', RenderKind.values),
  digest: r.strOrNull('digest'),
  files: r.list('files', _str),
  migration: r.objOrNull(
    'migration',
    (r) => MigrationSpec(
      file: r.str('file'),
      from: r.integer('from'),
      to: r.integer('to'),
    ),
  ),
);

Map<String, Object?> _renderOutcome(RenderOutcome o) => {
  'clean': o.clean,
  'data': _json(o.data),
};

RenderOutcome _renderOutcomeOf(_R r) =>
    RenderOutcome(clean: r.boolean('clean'), data: r.json('data'));

// ------------------------------------------------------------ jobs

Map<String, Object?> _job(Job j) => {
  'id': j.id,
  'kind': j.kind.name,
  'state': j.state.name,
  'step': j.step == null ? null : _stepKey(j.step!),
  'rev': j.rev,
  'artifactId': j.artifactId,
  'title': j.title,
  'input': _jobInput(j.input),
  'plan': j.plan == null ? null : _plan(j.plan!),
  'carry': j.carry == null ? null : _carry(j.carry!),
  'error': j.error == null ? null : _jobError(j.error!),
  // A list: a step's key is not a string, and the order is the commits'.
  'steps': [
    for (final e in j.steps.entries)
      {'key': _stepKey(e.key), 'result': _stepResult(e.value)},
  ],
  'threads': {
    for (final e in j.threads.entries) e.key: [e.value.$1, e.value.$2],
  },
  'questions': {
    for (final e in j.questions.entries) e.key: _jobQuestion(e.value),
  },
};

Job _jobOf(_R r) {
  final steps = <StepKey, StepResult>{};
  final pairs = r.objs('steps', (s) => (s.obj('key', _stepKeyOf), s));
  for (var i = 0; i < pairs.length; i++) {
    final (key, s) = pairs[i];
    if (steps.containsKey(key)) {
      throw _Fail(
        CodecErrorKind.malformed,
        '${r.at('steps')}[$i].key',
        'step $key named twice',
      );
    }
    steps[key] = s.obj('result', _stepResultOf);
  }
  return Job(
    id: r.str('id'),
    kind: r.enumOf('kind', JobKind.values),
    state: r.enumOf('state', JobState.values),
    step: r.objOrNull('step', _stepKeyOf),
    rev: r.integer('rev'),
    artifactId: r.strOrNull('artifactId'),
    title: r.strOrNull('title'),
    input: r.obj('input', _jobInputOf),
    plan: r.objOrNull('plan', _planOf),
    carry: r.objOrNull('carry', _carryOf),
    error: r.objOrNull('error', _jobErrorOf),
    steps: steps,
    threads: r.dict('threads', (v, p) {
      if (v case [final String comment, final bool stopped]) {
        return (comment, stopped);
      }
      throw _Fail(
        CodecErrorKind.malformed,
        p,
        'not [comment ID, stopped]: ${_kindOf(v)}',
      );
    }),
    questions: r.dict('questions', (v, p) => _jobQuestionOf(_R.of(v, p))),
  );
}

/// A step's key by its parts, not its text, so a key decodes as the type
/// it was (an `UnknownKey('reply')` is not read back as a `ReplyKey`).
Map<String, Object?> _stepKey(StepKey k) => switch (k) {
  TurnKey(:final turn) => {'t': 'turn', 'turn': turn},
  PublishKey(:final base) => {'t': 'publish', 'base': base},
  ReplyKey() => {'t': 'reply'},
  UnknownKey(:final raw) => {'t': 'unknown', 'raw': raw},
};

StepKey _stepKeyOf(_R r) => r.variant<StepKey>({
  'turn': (r) => TurnKey(r.integer('turn')),
  'publish': (r) => PublishKey(r.integer('base')),
  'reply': (_) => const ReplyKey(),
  'unknown': (r) => UnknownKey(r.str('raw')),
});

Map<String, Object?> _stepResult(StepResult s) => switch (s) {
  TurnStepResult(:final notes, :final base) => {
    't': 'turn',
    'notes': _notes(notes),
    'base': base,
  },
  PublishStepResult(:final artifactId, :final version) => {
    't': 'publish',
    'artifactId': artifactId,
    'version': version,
  },
  ReplyStepResult(:final data) => {'t': 'reply', 'data': _json(data)},
};

StepResult _stepResultOf(_R r) => r.variant<StepResult>({
  'turn': (r) =>
      TurnStepResult(r.obj('notes', _notesOf), base: r.integer('base')),
  'publish': (r) => PublishStepResult(
    artifactId: r.str('artifactId'),
    version: r.integer('version'),
  ),
  'reply': (r) => ReplyStepResult(data: r.json('data')),
});

Map<String, Object?> _jobInput(JobInput i) => {
  'request': i.request,
  'base': i.base,
  'threads': {...i.threads},
};

JobInput _jobInputOf(_R r) => JobInput(
  request: r.str('request'),
  base: r.integer('base'),
  threads: r.dict('threads', _str),
);

Map<String, Object?> _plan(PlanState p) => {
  'planRest': p.planRest,
  'data': _json(p.data),
};

PlanState _planOf(_R r) =>
    PlanState(planRest: r.boolean('planRest'), data: r.json('data'));

Map<String, Object?> _carry(Carry c) => {
  'stops': c.stops,
  'canKeepGoing': c.canKeepGoing,
  'data': _json(c.data),
};

Carry _carryOf(_R r) => Carry(
  stops: r.integer('stops'),
  canKeepGoing: r.boolean('canKeepGoing'),
  data: r.json('data'),
);

Map<String, Object?> _jobError(JobError e) => {
  'code': e.code,
  'detail': _json(e.detail),
  'step': e.step == null ? null : _stepKey(e.step!),
};

JobError _jobErrorOf(_R r) =>
    JobError(r.str('code'), r.json('detail'), r.objOrNull('step', _stepKeyOf));

Map<String, Object?> _jobQuestion(JobQuestion q) => {
  'answer': switch (q.answer) {
    AnswerPending() => {'t': 'pending'},
    Answered(:final answers) => {'t': 'answered', 'answers': _json(answers)},
    Declined() => {'t': 'declined'},
    Withdrawn() => {'t': 'withdrawn'},
  },
  'toolUseId': q.toolUseId,
};

JobQuestion _jobQuestionOf(_R r) => JobQuestion(
  r.obj(
    'answer',
    (a) => a.variant<JobAnswer>({
      'pending': (_) => const AnswerPending(),
      'answered': (a) => Answered(a.json('answers')),
      'declined': (_) => const Declined(),
      'withdrawn': (_) => const Withdrawn(),
    }),
  ),
  toolUseId: r.strOrNull('toolUseId'),
);

// ------------------------------------------------------------ turns and calls

Map<String, Object?> _notes(TurnNotes n) => {
  'calls': [for (final c in n.calls) _toolCall(c)],
  'data': _json(n.data),
};

TurnNotes _notesOf(_R r) =>
    TurnNotes(calls: r.objs('calls', _toolCallOf), data: r.json('data'));

Map<String, Object?> _toolCall(ToolCall c) => switch (c) {
  FinishCall(:final id, :final publish, :final reply) => {
    't': 'finish',
    'id': id,
    'publish': _publishPlan(publish),
    'reply': reply == null ? null : _replyPlan(reply),
  },
  RenderCall(:final id, :final spec) => {
    't': 'render',
    'id': id,
    'spec': _renderSpec(spec),
  },
  AskCall(:final id, :final question) => {
    't': 'ask',
    'id': id,
    'question': _question(question),
  },
  OtherCall() => {'t': 'other', ..._otherCall(c)},
};

ToolCall _toolCallOf(_R r) => r.variant<ToolCall>({
  'finish': (r) => FinishCall(
    r.str('id'),
    r.obj('publish', _publishPlanOf),
    reply: r.objOrNull('reply', _replyPlanOf),
  ),
  'render': (r) => RenderCall(r.str('id'), r.obj('spec', _renderSpecOf)),
  'ask': (r) => AskCall(r.str('id'), r.obj('question', _questionOf)),
  'other': _otherCallOf,
});

Map<String, Object?> _otherCall(OtherCall c) => {
  'id': c.id,
  'name': c.name,
  'args': _json(c.args),
};

OtherCall _otherCallOf(_R r) =>
    OtherCall(r.str('id'), r.str('name'), args: r.json('args'));

Map<String, Object?> _toolResult(ToolResult t) => {
  'data': _json(t.data),
  'isError': t.isError,
};

ToolResult _toolResultOf(_R r) =>
    ToolResult(data: r.json('data'), isError: r.boolean('isError'));

Map<String, Object?> _publishPlan(PublishPlan p) => {
  'base': p.base,
  'data': _json(p.data),
};

PublishPlan _publishPlanOf(_R r) =>
    PublishPlan(base: r.integer('base'), data: r.json('data'));

Map<String, Object?> _replyPlan(ReplyPlan p) => {'data': _json(p.data)};

ReplyPlan _replyPlanOf(_R r) => ReplyPlan(data: r.json('data'));

Map<String, Object?> _question(Question q) => {'data': _json(q.data)};

Question _questionOf(_R r) => Question(data: r.json('data'));

Map<String, Object?> _turnInput(TurnInput i) => {
  'turn': i.turn,
  'input': _jobInput(i.input),
  'base': i.base,
  'previous': i.previous == null ? null : _notes(i.previous!),
  'results': {for (final e in i.results.entries) e.key: _toolResult(e.value)},
  'carry': i.carry == null ? null : _carry(i.carry!),
  'planRest': i.planRest,
};

TurnInput _turnInputOf(_R r) => TurnInput(
  turn: r.integer('turn'),
  input: r.obj('input', _jobInputOf),
  base: r.integer('base'),
  previous: r.objOrNull('previous', _notesOf),
  results: r.dict('results', (v, p) => _toolResultOf(_R.of(v, p))),
  carry: r.objOrNull('carry', _carryOf),
  planRest: r.boolean('planRest'),
);

Map<String, Object?> _turnOutcome(TurnOutcome o) => {
  'notes': _notes(o.notes),
  'stop': switch (o.stop) {
    null => null,
    final s => {'canKeepGoing': s.canKeepGoing, 'data': _json(s.data)},
  },
};

TurnOutcome _turnOutcomeOf(_R r) => TurnOutcome(
  r.obj('notes', _notesOf),
  stop: r.objOrNull(
    'stop',
    (s) =>
        TurnStop(canKeepGoing: s.boolean('canKeepGoing'), data: s.json('data')),
  ),
);
