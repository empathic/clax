/// The app's scheme (spec §4.2 of
/// docs/superpowers/specs/2026-10-19-in-process-core-design.md): each
/// WebView request to the shell's host or an artifact's content host,
/// answered through the core.
library;

import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'core_client.dart';
import 'frame.dart';
import 'origins.dart';

/// One request a WebView asked the scheme for, as the native scheme host
/// (`SchemeHost.kt`, `SchemeHost.swift`) hands it over.
class SchemeRequest {
  const SchemeRequest({
    required this.url,
    required this.method,
    required this.headers,
    required this.mainFrame,
    this.navigation,
  });

  final Uri url;
  final String method;

  /// The request's headers, by any case.
  final Map<String, String> headers;

  /// Whether the request is the main frame's (on Apple, only a numbered
  /// one is).
  final bool mainFrame;

  /// The number of the main-frame navigation this request is the
  /// document of (spec §4.4); null for every other request, so for every
  /// request a frame inside the page makes.
  final int? navigation;

  @override
  String toString() => 'SchemeRequest($method $url, navigation: $navigation)';
}

/// The scheme's answer to one request.
class SchemeAnswer {
  const SchemeAnswer(this.status, this.headers, this.body, {this.nonce});

  final int status;

  /// Lowercase names; a repeated header's values joined.
  final Map<String, String> headers;
  final Uint8List body;

  /// The nonce written into a shell document's `clax-nonce` meta, which
  /// joins its navigation's set (spec §4.4); null for every other answer.
  final String? nonce;

  @override
  String toString() =>
      'SchemeAnswer($status, ${body.length} bytes${nonce == null ? '' : ', nonce'})';
}

/// Answers one scheme request (spec §4.2) through the core:
///
/// - `GET` and `HEAD` only, else 405.
/// - The shell's host is a `shell-page` frame, an artifact's content host
///   (`<id>.c`) a `content` frame naming its ID; any other host 404.
/// - A shell document (`/`, `/inbox`, `/people`, `/a/…` on the shell's
///   host) only for a request that carries a navigation number, and a
///   numbered request only for `/a/…`: else 403, before the core.
/// - Of the request's headers only `Range`, `Accept` and
///   `Service-Worker` go to the core; never `If-None-Match` or
///   `If-Modified-Since`, since a 304 is a 3xx no WebView here takes.
/// - A 3xx the core answers is followed in the core when its `Location`
///   is the request's own origin and directory (as `…/index.html` → `…/`
///   is), at most 5 hops, and the target's answer is the request's; a
///   document request for `/v/<n>` answered 308 is a page that refreshes
///   to its `Location`; any other 3xx is a 404.
/// - The answer loses every `Access-Control-*` header and its length.
/// - A shell document answered 200 to a numbered request gets a fresh
///   nonce, written as `<meta name="clax-nonce" content="…">` right after
///   its first literal `<head>`, and named in [SchemeAnswer.nonce]. One
///   with no `<head>` goes unchanged with no nonce, so its load has no
///   channel, and [onFault] hears it.
class SchemeAnswerer {
  SchemeAnswerer(this.core, this.origins, {this.onFault, Random? random})
    : _random = random ?? Random.secure();

  final CoreCalls core;
  final AppOrigins origins;
  final Random _random;

  /// A shell document the answerer could not give a nonce.
  final void Function(String detail)? onFault;

  /// The request headers that reach the core.
  static const forwarded = ['range', 'accept', 'service-worker'];

  /// The most redirects followed for one request.
  static const maxHops = 5;

  Future<SchemeAnswer> answer(SchemeRequest r) async {
    final method = r.method.toUpperCase();
    if (method != 'GET' && method != 'HEAD') {
      return _text(405, "the app's scheme answers GET and HEAD only");
    }
    final url = r.url;
    final shell = _isShell(url);
    final id = shell ? null : _contentId(url);
    if (!shell && id == null) return _text(404, 'not one of the app\'s hosts');
    final path = url.path.isEmpty ? '/' : url.path;
    final document = shell && path.startsWith('/a/');
    if (shell) {
      final numbered = r.navigation != null;
      if (numbered ? !document : (document || _galleryPaths.contains(path))) {
        return _text(403, 'refused');
      }
    }
    final headers = <(String, String)>[
      for (final name in forwarded)
        if (_header(r.headers, name) case final v?) (name, v),
    ];
    final as = shell ? CallAs.shellPage : CallAs.content;
    var target = url;
    for (var hop = 0; ; hop++) {
      final call = core.call(
        CoreRequest(
          method,
          _pathOf(target),
          headers: headers,
          as: as,
          host: id,
        ),
      );
      final CoreResponse res;
      switch (await call.answer) {
        case CoreAnswered(:final response):
          res = response;
        case CoreUnanswered(:final code, :final message):
          return _text(502, 'the core did not answer: $code: $message');
      }
      if (res.status < 300 || res.status >= 400) {
        return _answer(res, method, document && r.navigation != null);
      }
      final location = res.header('location');
      final next = location == null ? null : _resolve(target, location);
      if (next != null && hop < maxHops && _sameBase(url, next)) {
        target = next;
        continue;
      }
      if (res.status == 308 &&
          location != null &&
          hop == 0 &&
          _isVersionRoot(path) &&
          (r.mainFrame || _wantsHtml(r.headers))) {
        return _refresh(location);
      }
      return _text(404, 'not found');
    }
  }

  SchemeAnswer _answer(CoreResponse res, String method, bool shellDocument) {
    final headers = <String, String>{};
    for (final (k, v) in res.headers) {
      final name = k.toLowerCase();
      if (name.startsWith('access-control-') || name == 'content-length') {
        continue;
      }
      final had = headers[name];
      headers[name] = had == null
          ? v
          : '$had${name == 'set-cookie' ? '\n' : ', '}$v';
    }
    var body = res.body;
    String? nonce;
    if (shellDocument && res.status == 200) {
      final minted = _nonce();
      final written = _withNonce(body, minted);
      if (written != null) {
        nonce = minted;
        body = written;
      } else {
        onFault?.call(
          'the shell document has no <head> for its clax-nonce meta, so its '
          'load has no channel',
        );
      }
    }
    if (method == 'HEAD') body = Uint8List(0);
    return SchemeAnswer(res.status, headers, body, nonce: nonce);
  }

  /// [body] with the nonce meta right after its first `<head>`; null when
  /// it has none.
  static Uint8List? _withNonce(Uint8List body, String nonce) {
    final at = _indexOf(body, _head);
    if (at < 0) return null;
    final meta = utf8.encode('<meta name="clax-nonce" content="$nonce">');
    final end = at + _head.length;
    return Uint8List.fromList([
      ...body.sublist(0, end),
      ...meta,
      ...body.sublist(end),
    ]);
  }

  static final _head = utf8.encode('<head>');

  static int _indexOf(Uint8List hay, List<int> needle) {
    outer:
    for (var i = 0; i + needle.length <= hay.length; i++) {
      for (var j = 0; j < needle.length; j++) {
        if (hay[i + j] != needle[j]) continue outer;
      }
      return i;
    }
    return -1;
  }

  /// 24 random bytes in unpadded base64url: 32 of `[A-Za-z0-9_-]`.
  String _nonce() => base64Url
      .encode(List<int>.generate(24, (_) => _random.nextInt(256)))
      .replaceAll('=', '');

  /// The gallery's documents, which the app never loads.
  static const _galleryPaths = {'/', '/inbox', '/people'};

  bool _isShell(Uri u) {
    final shell = Uri.parse(origins.shell);
    return u.scheme == shell.scheme &&
        u.host.toLowerCase() == shell.host &&
        u.port == shell.port;
  }

  String? _contentId(Uri u) {
    final template = Uri.parse(origins.content('x'));
    if (u.scheme != template.scheme || u.port != template.port) return null;
    return origins.idOfHost(u.host);
  }

  static String _pathOf(Uri u) =>
      '${u.path.isEmpty ? '/' : u.path}${u.hasQuery ? '?${u.query}' : ''}';

  static String? _header(Map<String, String> headers, String name) {
    for (final MapEntry(:key, :value) in headers.entries) {
      if (key.toLowerCase() == name) return value;
    }
    return null;
  }

  static Uri? _resolve(Uri base, String location) {
    try {
      return base.resolve(location);
    } on FormatException {
      return null;
    }
  }

  /// Whether [to] is [from]'s origin and directory (its path up to its
  /// last `/`), so the target's relative links resolve as the request's.
  static bool _sameBase(Uri from, Uri to) =>
      from.scheme == to.scheme &&
      from.host.toLowerCase() == to.host.toLowerCase() &&
      from.port == to.port &&
      _dir(from.path) == _dir(to.path);

  static String _dir(String path) =>
      path.substring(0, path.lastIndexOf('/') + 1);

  /// `/v/<n>`, which the content routes redirect to `/v/<n>/`.
  static bool _isVersionRoot(String path) =>
      RegExp(r'^/v/[0-9]+$').hasMatch(path);

  static bool _wantsHtml(Map<String, String> headers) =>
      (_header(headers, 'accept') ?? '').toLowerCase().contains('text/html');

  static SchemeAnswer _refresh(String location) => SchemeAnswer(
    200,
    const {'content-type': 'text/html; charset=utf-8'},
    utf8.encode(
      '<meta http-equiv="refresh" content="0;url=${_escape(location)}">',
    ),
  );

  static String _escape(String s) => s
      .replaceAll('&', '&amp;')
      .replaceAll('"', '&quot;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;');

  static SchemeAnswer _text(int status, String text) => SchemeAnswer(
    status,
    const {'content-type': 'text/plain; charset=utf-8'},
    utf8.encode(text),
  );
}
