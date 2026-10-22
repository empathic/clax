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

  /// Loads [url] in the inspect view and runs the draft's test [files]
  /// (`tests/<name>.test.js`) there with the page's test harness, each
  /// test for at most [perTest], the run for at most [total].
  ///
  /// Throws [InspectorError] when the page does not load, the harness
  /// does not answer in time, or [cancelled] completes first.
  Future<TestRun> runTests(
    Uri url, {
    required List<String> files,
    Duration perTest = const Duration(seconds: 5),
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
  });

  /// Loads [url] in the inspect view and runs the migration [file] (an app
  /// bundle's `migrate.js`) there: its default export is called with the
  /// page's `db` and `{from, to}` (the installed version and the bundle's),
  /// the whole run for at most [total].
  ///
  /// The page's own markup does not run: only the migration writes the
  /// documents. Once this returns, whatever the outcome, no code of the
  /// run is running any more.
  ///
  /// A migration that throws, does not load or runs out of time is a
  /// [MigrationRun] that is not [MigrationRun.ok]. Throws [InspectorError]
  /// when the page does not load, the page does not answer in time, or
  /// [cancelled] completes first.
  ///
  /// With [onWrite] the run is journaled (an update's live run, bundles
  /// spec §8.4): before each document's first write, through any `db`
  /// handle of the page, [onWrite] gets its value, and the write goes ahead
  /// only once [onWrite] completes (it fails when [onWrite] throws). Writes
  /// after the run's answer fail.
  Future<MigrationRun> runMigration(
    Uri url, {
    String file = 'migrate.js',
    required int from,
    required int to,
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
    Future<void> Function(MigrationWrite write)? onWrite,
  });

  /// Loads [url] in the migration view and puts back [writes] (a journaled
  /// run's, in the order [runMigration] gave them) in reverse order, as the
  /// person's viewer, so their private `data/users/<id>/` documents too: a
  /// document whose `before` is null is deleted, any other set to it.
  /// Throws [InspectorError] when any of it failed, the page did not load
  /// or answer within [total], or [cancelled] completes first.
  Future<void> restoreWrites(
    Uri url,
    List<MigrationWrite> writes, {
    Duration total = const Duration(seconds: 60),
    Future<void>? cancelled,
  });
}

/// One entry of a journaled migration: the document at [path] held
/// [before] (null: there was none) before the migration's first write to
/// it.
class MigrationWrite {
  const MigrationWrite(this.path, this.before);

  final String path;
  final Map<String, Object?>? before;

  Map<String, Object?> toJson() => {'path': path, 'before': before};

  static MigrationWrite? fromJson(Object? j) {
    if (j is! Map || j['path'] is! String) return null;
    final b = j['before'];
    if (b != null && b is! Map) return null;
    return MigrationWrite(
      j['path'] as String,
      b == null ? null : (b as Map).cast<String, Object?>(),
    );
  }
}

/// What a run of an app bundle's migration found: it finished ([ok]), or
/// [error], why not.
class MigrationRun {
  const MigrationRun({required this.ok, this.error});
  const MigrationRun.passed() : ok = true, error = null;
  const MigrationRun.failed(String this.error) : ok = false;

  final bool ok;

  /// Why the migration did not finish, when it did not.
  final String? error;
}

/// What a run of a draft's tests found.
class TestRun {
  const TestRun({
    required this.passed,
    required this.failed,
    required this.skipped,
  });

  /// How many tests passed.
  final int passed;

  /// Each test that failed, and each test file that did not load, with
  /// why.
  final List<({String name, String error})> failed;

  /// How many tests did not run because the run's time ran out.
  final int skipped;

  /// The run as the `render` tool reports it.
  Map<String, Object?> toJson() => {
    'passed': passed,
    'failed': [
      for (final f in failed) {'name': f.name, 'error': f.error},
    ],
    'skipped': skipped,
  };
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
