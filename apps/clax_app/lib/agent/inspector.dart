/// What loads a draft page and reports what it shows: the eyes of the
/// `render` tool.
library;

import 'dart:typed_data';

/// Loads a page of the daemon in the inspect view (`?app=1&inspect=1`)
/// and reports what it shows.
///
/// An implementation serves one inspection at a time (a later call waits
/// for the earlier one) and may reuse one page view for every call.
abstract interface class Inspector {
  /// Loads [url] and returns what it shows once the page has been quiet
  /// for a moment, at most [settle] after it loaded, with a screenshot of
  /// at most [maxBytes].
  ///
  /// Throws [InspectorError] when the page does not load or answer in
  /// time, or when [cancelled] completes first.
  Future<Inspection> inspect(
    Uri url, {
    Duration settle = const Duration(seconds: 3),
    int maxBytes = 1 << 20,
    Future<void>? cancelled,
  });
}

/// What an inspected page showed.
class Inspection {
  const Inspection({
    required this.png,
    required this.errors,
    required this.failed,
    required this.text,
    required this.title,
    required this.height,
    required this.settledMs,
  });

  /// The viewport as a PNG.
  final Uint8List png;

  /// What the page logged since its first script, oldest first: `kind` is
  /// `error` (an uncaught error), `unhandledrejection` or `console` (a
  /// `console.error` call).
  final List<({String kind, String text})> errors;

  /// The URLs of resources that failed to load.
  final List<String> failed;

  /// An outline of the visible page: headings, buttons, labels, inputs.
  final String text;

  /// The document's title.
  final String title;

  /// The document's scroll height in CSS pixels.
  final int height;

  /// How long the page took to settle, in milliseconds.
  final int settledMs;

  /// The [errors] a page must not have to count as working: uncaught
  /// errors and unhandled rejections, not `console.error` calls.
  Iterable<({String kind, String text})> get uncaught =>
      errors.where((e) => e.kind == 'error' || e.kind == 'unhandledrejection');
}

/// An inspection that did not happen: the page did not load, did not
/// answer in time, or the run was cancelled. [message] says which, for the
/// agent.
class InspectorError implements Exception {
  const InspectorError(this.message);

  final String message;

  @override
  String toString() => 'InspectorError: $message';
}
