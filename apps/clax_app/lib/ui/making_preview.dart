import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:webview_flutter/webview_flutter.dart';

import '../platform/clock.dart';

/// A view that shows HTML handed to it whole, for the making screen's live
/// preview of a new app's `index.html` as it is written.
abstract interface class PreviewView {
  /// Shows [html], replacing what was shown.
  Future<void> loadHtml(String html);

  /// The view, sized by its parent.
  Widget build(BuildContext context);

  void dispose();
}

/// The Content-Security-Policy every preview document carries: no network
/// of any kind; inline styles and `data:` images and fonts only.
const previewPolicy =
    "default-src 'none'; style-src 'unsafe-inline'; " // copy-check: allow
    'img-src data: blob:; font-src data:; media-src data:';

final _doctype = RegExp(r'^\s*<!doctype[^>]*>', caseSensitive: false);

/// [html] with a `<meta>` carrying [previewPolicy] placed before any of
/// its content: right after a leading doctype (so the document keeps its
/// standards mode), else at the very start.
String previewDocument(String html) {
  const meta =
      '<meta http-equiv="Content-Security-Policy" content="$previewPolicy">';
  final m = _doctype.firstMatch(html);
  if (m == null) return '$meta$html';
  return '${html.substring(0, m.end)}$meta${html.substring(m.end)}';
}

/// Whether the preview may navigate to [url]: only its own loaded
/// documents (`about:blank`, and `about:srcdoc` for frames), never the
/// network, a file or a `data:` document (which would leave the
/// [previewPolicy] behind).
bool previewAllows(String url) => url == 'about:blank' || url == 'about:srcdoc';

/// Whether a preview WebView on [platform] takes a background colour:
/// WKWebView on macOS throws on `setOpaque`, which that call uses.
bool previewSetsBackground(TargetPlatform platform) =>
    platform != TargetPlatform.macOS;

/// [PreviewView] over a platform WebView: [WebViewController.loadHtmlString]
/// with JavaScript off, every navigation but its own documents refused
/// ([previewAllows]), the network shut by [previewPolicy], and no touch
/// input (a preview that reloads several times a second is not used).
class WebViewPreview implements PreviewView {
  WebViewPreview({Color? background, WebViewController? controller})
    : controller = controller ?? WebViewController() {
    final c = this.controller;
    c
      ..setJavaScriptMode(JavaScriptMode.disabled)
      ..setNavigationDelegate(
        NavigationDelegate(
          onNavigationRequest: (r) => previewAllows(r.url)
              ? NavigationDecision.navigate
              : NavigationDecision.prevent,
        ),
      );
    if (background != null && previewSetsBackground(defaultTargetPlatform)) {
      // Not every platform view takes a background colour, and some throw
      // before returning a future.
      try {
        c.setBackgroundColor(background).catchError((Object _) {});
      } on Object catch (_) {}
    }
  }

  /// Whether this platform has a WebView (a widget test has none).
  static bool get available => WebViewPlatform.instance != null;

  final WebViewController controller;

  @override
  Future<void> loadHtml(String html) =>
      controller.loadHtmlString(previewDocument(html));

  @override
  Widget build(BuildContext context) =>
      IgnorePointer(child: WebViewWidget(controller: controller));

  @override
  void dispose() {}
}

/// Hands the latest of the texts [offer]ed to [load], at most once per
/// [interval] on [clock]: the first at once, later ones as soon as the
/// interval since the last load has passed, and in between only the newest
/// (a text offered while one waits replaces it).
class PreviewThrottle {
  PreviewThrottle({
    required this.load,
    this.clock = const SystemClock(),
    this.interval = const Duration(milliseconds: 250),
  });

  final void Function(String text) load;
  final Clock clock;
  final Duration interval;

  DateTime? _last;
  String? _pending;
  bool _waiting = false;
  bool _closed = false;

  /// Loads [text] now if the interval allows, else when it does.
  void offer(String text) {
    if (_closed) return;
    _pending = text;
    if (_waiting) return;
    final last = _last;
    final now = clock.now();
    if (last == null || now.difference(last) >= interval) {
      _fire(now);
      return;
    }
    _waiting = true;
    unawaited(
      clock.delay(interval - now.difference(last)).then((_) {
        _waiting = false;
        // Offered again, so a flush in the meantime still spaces the loads.
        final p = _pending;
        if (p != null) offer(p);
      }),
    );
  }

  /// Loads the waiting text now, if there is one.
  void flush() {
    if (!_closed && _pending != null) _fire(clock.now());
  }

  /// Drops the waiting text: an offer after this starts afresh.
  void clear() => _pending = null;

  void close() {
    _closed = true;
    _pending = null;
  }

  void _fire(DateTime now) {
    final text = _pending!;
    _pending = null;
    _last = now;
    load(text);
  }
}
