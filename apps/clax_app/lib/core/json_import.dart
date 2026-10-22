/// The one-time import of the app's JSON state files into the core's jobs
/// (docs/contract.md, "The
/// app's JSON state, imported once").
library;

import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import '../clax/api.dart' show ClaxApiError;
import '../clax/transport.dart';
import '../platform/clock.dart';

/// The key of the import: `POST /api/client-imports/app-json-v1`.
const jsonImportKey = 'app-json-v1';

/// The state files the import reads, by the names the app keeps them under
/// in its support directory (crates/clax-store/src/imports.rs
/// `APP_JSON_FILES`).
const jsonStateFiles = <String>[
  'pending-make.json',
  'plans.json',
  'unfinished.json',
  'change-ledger.json',
  'asked.json',
  'stopped.json',
  'agent-read-consent.json',
];

/// Where the imported files are kept, under the support directory: one
/// directory per import, named by its time.
const importedJsonDir = 'imported-json';

/// What an import did, as the core answered it (`{import: {already, jobs,
/// consents, skipped, left_out, done_at}}`), and where the files it read
/// were copied.
final class ImportSummary {
  const ImportSummary({
    required this.already,
    required this.jobs,
    required this.consents,
    required this.skipped,
    required this.leftOut,
    required this.doneAt,
    required this.copiedTo,
  });

  /// The answer's `import` object; throws [FormatException] for any other
  /// shape.
  factory ImportSummary.fromAnswer(Object? json, {required String copiedTo}) {
    List<String> strings(Object? v, String key) => switch (v) {
      final List<Object?> l when l.every((s) => s is String) =>
        l.cast<String>().toList(growable: false),
      _ => throw FormatException('$key is not a list of strings'),
    };
    return switch (json) {
      {
        'import': {
          'already': final bool already,
          'jobs': final int jobs,
          'consents': final int consents,
          'skipped': final Object? skipped,
          'left_out': final Object? leftOut,
          'done_at': final String doneAt,
        },
      } =>
        ImportSummary(
          already: already,
          jobs: jobs,
          consents: consents,
          skipped: strings(skipped, 'skipped'),
          leftOut: strings(leftOut, 'left_out'),
          doneAt: doneAt,
          copiedTo: copiedTo,
        ),
      _ => throw const FormatException('not an import summary'),
    };
  }

  /// The key was imported before: the core changed nothing, and the rest
  /// is that import's summary.
  final bool already;

  /// The jobs and read consents the import made.
  final int jobs;
  final int consents;

  /// The files that did not parse.
  final List<String> skipped;

  /// The entries left out because they name what the core no longer
  /// holds, each `<file>: <why>`.
  final List<String> leftOut;

  /// When the import was made (the first one, when [already]).
  final String doneAt;

  /// The directory the files were copied to before they were deleted.
  final String copiedTo;

  @override
  String toString() =>
      'ImportSummary(${already ? 'already, ' : ''}$jobs jobs, '
      '$consents consents, skipped $skipped, left out $leftOut, '
      'done at $doneAt, copied to $copiedTo)';
}

/// Imports whichever of [jsonStateFiles] exist in [supportDir] once: each
/// is read as UTF-8 and sent as it is, `POST
/// /api/client-imports/app-json-v1` `{client: "app", files}` with the
/// queue owner's [ownerToken] in `x-clax-jobs-owner`; the core parses
/// them. On any 200 (a first import, or `already`) the files are copied,
/// as they were read, to `<supportDir>/imported-json/<time>/` (the time
/// [clock]'s, in UTC) and then deleted by [delete]. Answers the summary,
/// or null when no file exists (no call is made).
///
/// The core's import is one transaction and answers a key imported before
/// with `already: true`, so a run cut off after the core's commit (before
/// or during the deletes) is harmless: the next run sends what is left,
/// is answered `already`, and deletes it.
///
/// Throws [ClaxApiError] when the core refuses the import (its status and
/// code) or no answer came (status 0), and [FileSystemException] when a
/// file cannot be read, copied or deleted; the files not yet deleted stay
/// for the next run. A 200 whose body is no summary throws
/// [FormatException] after the copies and deletes.
Future<ImportSummary?> importJsonState({
  required String supportDir,
  required ClaxTransport core,
  required String ownerToken,
  Clock clock = const SystemClock(),
  Duration timeout = const Duration(seconds: 125),
  void Function(File file) delete = _delete,
}) async {
  final read = <String, Uint8List>{};
  for (final name in jsonStateFiles) {
    final f = File('$supportDir/$name');
    if (f.existsSync()) read[name] = f.readAsBytesSync();
  }
  if (read.isEmpty) return null;
  final body = jsonEncode({
    'client': 'app',
    'files': {
      for (final e in read.entries)
        e.key: utf8.decode(e.value, allowMalformed: true),
    },
  });
  final a = await core.send(
    'POST',
    '/api/client-imports/$jsonImportKey',
    headers: {
      'content-type': 'application/json',
      'x-clax-jobs-owner': ownerToken,
    },
    body: Uint8List.fromList(utf8.encode(body)),
    timeout: timeout,
  );
  final Object? json;
  try {
    json = a.body.isEmpty ? null : jsonDecode(utf8.decode(a.body));
  } on FormatException {
    if (a.status != 200) throw _refused(a.status, null);
    rethrow;
  }
  if (a.status != 200) throw _refused(a.status, json);
  final stamp = clock.now().toUtc().toIso8601String().replaceAll(':', '-');
  final dir = Directory('$supportDir/$importedJsonDir/$stamp')
    ..createSync(recursive: true);
  for (final e in read.entries) {
    File('${dir.path}/${e.key}').writeAsBytesSync(e.value, flush: true);
  }
  for (final name in read.keys) {
    delete(File('$supportDir/$name'));
  }
  return ImportSummary.fromAnswer(json, copiedTo: dir.path);
}

void _delete(File f) {
  if (f.existsSync()) f.deleteSync();
}

ClaxApiError _refused(int status, Object? json) {
  final error = switch (json) {
    {'error': final Map<Object?, Object?> e} => e.cast<String, Object?>(),
    _ => const <String, Object?>{},
  };
  return ClaxApiError(
    status: status,
    code: switch (error['code']) {
      final String c => c,
      _ => 'http_$status',
    },
    message: switch (error['message']) {
      final String m => m,
      _ => 'the import was refused',
    },
    body: error,
  );
}
