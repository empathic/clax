/// The host's half of the shell's app transport (`web/shell/src/transport.ts`):
/// the shell's API calls and its event stream, relayed through the core.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'core_client.dart';
import 'frame.dart';

/// Relays one shell document's `ClaxHost` messages of type `call`,
/// `callChunk`, `callAbort`, `subscribe` and `unsubscribe` through the core,
/// and answers each into the document by `window.claxTransport`, naming
/// the nonce the message it answers carried as its first argument. The
/// surface hands it only messages of the load's own nonce (spec §4.4) and
/// makes a new relay for each load, disposing the last.
///
/// - `call {id, method, path, headers, body | body64, chunked?}` goes to
///   the core as an `as: owner-browser` frame (the owner's browser, as a
///   browser's shell is) with the message's headers but `Host`; a `chunked` call waits for its `callChunk {id, n, last,
///   body64}`s, `n` from 0 (one out of order is dropped). A call naming
///   `site` (an artifact ID) is a content frame's call to its own site's API
///   that the shell relays (`web/shell/src/caps/site.ts`): it goes as an
///   `as: site` frame for that site, which the core limits to the site's
///   `/_clax/api/` and gives the site's cookies and origin, never the
///   owner's. The answer is
///   `claxTransport.callResult(nonce, id, {status, headers, body |
///   body64})`: `body` for a text or JSON answer that is valid UTF-8, else
///   `body64`. An answer of more than [partBytes] goes in parts, all as
///   base64: `callResult(nonce, id, {body64, more: true})` for each
///   [partBytes] of it but the last, then the ordinary `callResult` with
///   its status, headers and the rest as `body64`; the parts' base64 texts
///   join into the whole body's. Each part is encoded only once the one
///   before it has run, so one part's base64 is held at a time; a
///   `callAbort`, [dispose], or a part the document could not run stops
///   the rest. A call the core could not answer is
///   answered 503 with the core's `{"error": {code, message}}` when it is
///   closed or failed, else 502.
/// - `callAbort {id}` cancels the call in the core and drops its answer
///   (or the rest of its parts).
/// - `subscribe {id, lastEventId?}` opens a core subscription from
///   `lastEventId` and forwards each of its blocks by
///   `claxTransport.streamBlock(nonce, id, text)`, its closing `event: end`
///   block included, then `claxTransport.streamEnd(nonce, id)`.
///   `unsubscribe {id}` cancels it with nothing more run.
///
/// Scripts run through [runJs] in the order the answers come; one the
/// document can no longer run is dropped.
class ChannelRelay {
  ChannelRelay(this.core, {required this.runJs});

  final CoreCalls core;

  /// Runs an expression in the shell document.
  final Future<void> Function(String js) runJs;

  /// The most body bytes one script carries: a multiple of 3, so each
  /// part's base64 has no padding and the parts' texts join, and exactly
  /// 1 MiB of base64.
  static const partBytes = 3 << 18;

  /// The calls not yet answered, by ID.
  final Map<int, _Call> _calls = {};

  /// The open subscriptions, by ID.
  final Map<int, _Stream> _streams = {};
  bool _disposed = false;

  /// The message types this relay answers.
  static const types = {
    'call',
    'callChunk',
    'callAbort',
    'subscribe',
    'unsubscribe',
  };

  /// Handles one `ClaxHost` message of the relay's [types]; false for any
  /// other. A malformed one, or one after [dispose], is taken and dropped.
  bool handle(Map<String, Object?> msg) {
    final type = msg['type'];
    if (!types.contains(type)) return false;
    final nonce = msg['nonce'];
    final id = msg['id'];
    if (_disposed || nonce is! String || id is! int) return true;
    switch (type) {
      case 'call':
        _call(nonce, id, msg);
      case 'callChunk':
        _chunk(id, msg);
      case 'callAbort':
        final c = _calls.remove(id);
        if (c?.tag case final tag? when !c!.answered) core.cancel(tag);
      case 'subscribe':
        _subscribe(nonce, id, msg['lastEventId']);
      case 'unsubscribe':
        _streams.remove(id)?.sub.cancel();
    }
    return true;
  }

  /// Cancels the subscriptions and the calls in flight; nothing more is
  /// answered.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    for (final c in _calls.values) {
      if (c.tag case final tag? when !c.answered) core.cancel(tag);
    }
    _calls.clear();
    for (final s in _streams.values) {
      s.sub.cancel();
    }
    _streams.clear();
  }

  void _call(String nonce, int id, Map<String, Object?> msg) {
    final method = msg['method'];
    final path = msg['path'];
    if (method is! String || path is! String || _calls.containsKey(id)) {
      return;
    }
    final headers = <(String, String)>[
      if (msg['headers'] case final List<Object?> hs)
        for (final h in hs)
          if (h case [final String k, final String v]
              when k.toLowerCase() != 'host')
            (k, v),
    ];
    final site = msg['site'];
    if (site != null && (site is! String || !_artifactId.hasMatch(site))) {
      return;
    }
    final c = _Call(nonce, method, path, headers, site as String?);
    _calls[id] = c;
    if (msg['chunked'] == true) return;
    final Uint8List? body;
    try {
      body = switch ((msg['body64'], msg['body'])) {
        (final String b64, _) => base64Decode(b64),
        (_, final String text) => utf8.encode(text),
        _ => null,
      };
    } on FormatException {
      _calls.remove(id);
      return;
    }
    if (body != null) c.parts.add(body);
    _send(id, c);
  }

  void _chunk(int id, Map<String, Object?> msg) {
    final c = _calls[id];
    final b64 = msg['body64'];
    if (c == null || c.tag != null || b64 is! String) return;
    if (msg['n'] != c.parts.length) return;
    try {
      c.parts.add(base64Decode(b64));
    } on FormatException {
      return;
    }
    if (msg['last'] == true) _send(id, c);
  }

  void _send(int id, _Call c) {
    final bytes = BytesBuilder(copy: false);
    c.parts.forEach(bytes.add);
    c.parts.clear();
    final call = core.call(
      CoreRequest(
        c.method,
        c.path,
        headers: c.headers,
        body: bytes.isEmpty ? null : bytes.takeBytes(),
        as: c.site == null ? CallAs.ownerBrowser : CallAs.site,
        host: c.site,
      ),
    );
    c.tag = call.tag;
    unawaited(
      call.answer.then((a) async {
        if (_disposed || _calls[id] != c) return;
        c.answered = true;
        await _answer(id, c, a);
        if (_calls[id] == c) _calls.remove(id);
      }),
    );
  }

  Future<void> _answer(int id, _Call c, CoreAnswer a) async {
    final CoreResponse res;
    switch (a) {
      case CoreAnswered(:final response):
        res = response;
      case CoreUnanswered(code: 'cancelled'):
        return;
      case CoreUnanswered(:final code, :final message):
        res = CoreResponse(
          code == 'core_closed' || code == 'core_failed' ? 503 : 502,
          const [('content-type', 'application/json')],
          utf8.encode(
            jsonEncode({
              'error': {'code': code, 'message': message},
            }),
          ),
        );
    }
    final head = <String, Object?>{
      'status': res.status,
      'headers': [
        for (final (k, v) in res.headers) [k, v],
      ],
    };
    final body = res.body;
    final n = jsonEncode(c.nonce);
    if (body.length > partBytes) {
      var at = 0;
      for (; body.length - at > partBytes; at += partBytes) {
        final part = Uint8List.sublistView(body, at, at + partBytes);
        final ran = await _runNow(
          'callResult($n, $id, '
          '{"body64":${jsonEncode(base64Encode(part))},"more":true})',
        );
        if (!ran || _disposed || _calls[id] != c) return;
      }
      head['body64'] = base64Encode(Uint8List.sublistView(body, at));
    } else {
      final text = _textual.hasMatch(res.header('content-type') ?? '')
          ? _utf8(body)
          : null;
      head[text != null ? 'body' : 'body64'] = text ?? base64Encode(body);
    }
    _run('callResult($n, $id, ${jsonEncode(head)})');
  }

  void _subscribe(String nonce, int id, Object? lastEventId) {
    if (_streams.containsKey(id)) return;
    final sub = core.subscribe(
      lastEventId: lastEventId is String && lastEventId.isNotEmpty
          ? lastEventId
          : null,
    );
    final s = _Stream(sub);
    _streams[id] = s;
    final n = jsonEncode(nonce);
    sub.blocks.listen(
      (block) {
        if (_streams[id] == s) {
          _run('streamBlock($n, $id, ${jsonEncode(block)})');
        }
      },
      onDone: () {
        if (_streams[id] != s) return;
        _streams.remove(id);
        _run('streamEnd($n, $id)');
      },
    );
  }

  void _run(String call) => unawaited(_runNow(call));

  /// Runs [call]; whether the document ran it.
  Future<bool> _runNow(String call) async {
    if (_disposed) return false;
    try {
      await runJs('window.claxTransport.$call');
      return true;
    } on Object {
      return false;
    }
  }

  static String? _utf8(Uint8List b) {
    try {
      return utf8.decode(b);
    } on FormatException {
      return null;
    }
  }

  /// An artifact ID, as a `call`'s `site` names one.
  static final _artifactId = RegExp(r'^[0-9a-hjkmnp-tv-z]{12}$');

  /// The content types the shell's transport reads as text.
  static final _textual = RegExp(
    r'^(text/|application/([\w.+-]*\+)?json\b|application/x-www-form-urlencoded\b)',
    caseSensitive: false,
  );
}

class _Call {
  _Call(this.nonce, this.method, this.path, this.headers, this.site);

  final String nonce;
  final String method;
  final String path;
  final List<(String, String)> headers;

  /// The artifact whose site API the call is, for a site call.
  final String? site;
  final List<Uint8List> parts = [];

  /// The core call's tag, once sent.
  int? tag;

  /// Whether the core answered; its answer may still be going out in
  /// parts.
  bool answered = false;
}

class _Stream {
  _Stream(this.sub);

  final CoreSubscription sub;
}
