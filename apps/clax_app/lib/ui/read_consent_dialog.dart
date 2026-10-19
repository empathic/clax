/// The owner's consent for Clax to read another app's documents,
/// asked in a dialog and remembered per pair of apps when the owner says
/// so.
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

import '../agent/read_consent.dart';
import 'kit/kit.dart';

/// The pairs of apps whose reads the owner allowed always: the agent
/// working on `reader` may read `source`'s documents.
abstract interface class ConsentMemory {
  Future<bool> allowed(String reader, String source);
  Future<void> allow(String reader, String source);

  /// Every remembered `(reader, source)` pair.
  Future<List<(String, String)>> pairs();

  /// Forgets the pair, so the agent asks again; a pair not remembered is
  /// left as it is.
  Future<void> forget(String reader, String source);
}

/// [ConsentMemory] for the app's life only.
class MemoryConsentMemory implements ConsentMemory {
  final _pairs = <(String, String)>{};

  @override
  Future<bool> allowed(String reader, String source) async =>
      _pairs.contains((reader, source));

  @override
  Future<void> allow(String reader, String source) async =>
      _pairs.add((reader, source));

  @override
  Future<List<(String, String)>> pairs() async => _pairs.toList();

  @override
  Future<void> forget(String reader, String source) async =>
      _pairs.remove((reader, source));
}

/// [ConsentMemory] kept in [file] as `{"always": [[reader, source], …]}`.
/// A save writes a sibling temporary file and renames it over the old one;
/// a missing or broken file remembers nothing.
class FileConsentMemory implements ConsentMemory {
  FileConsentMemory(this.file);

  final File file;
  Set<(String, String)>? _pairs;

  Set<(String, String)> _load() => _pairs ??= () {
    try {
      final j = jsonDecode(file.readAsStringSync());
      if (j case {'always': final List<Object?> always}) {
        return {
          for (final p in always)
            if (p case [final String r, final String s]) (r, s),
        };
      }
    } on FileSystemException {
      // Never saved.
    } on FormatException {
      // Broken: remember nothing.
    }
    return <(String, String)>{};
  }();

  @override
  Future<bool> allowed(String reader, String source) async =>
      _load().contains((reader, source));

  @override
  Future<void> allow(String reader, String source) async {
    final pairs = _load();
    if (!pairs.add((reader, source))) return;
    _save(pairs);
  }

  @override
  Future<List<(String, String)>> pairs() async => _load().toList();

  @override
  Future<void> forget(String reader, String source) async {
    final pairs = _load();
    if (!pairs.remove((reader, source))) return;
    _save(pairs);
  }

  void _save(Set<(String, String)> pairs) {
    file.parent.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      jsonEncode({
        'always': [
          for (final (r, s) in pairs) [r, s],
        ],
      }),
      flush: true,
    );
    tmp.renameSync(file.path);
  }
}

/// [AgentReadConsent] in a `ClaxDialog` over the app: "Let Clax read
/// Runs?", its body naming the app Clax works on, with Don’t allow, **Allow
/// once** and an "Always for this app" checkbox (not for a new app, which
/// has no ID to remember it by); Allow once with it ticked answers
/// [ConsentAnswer.always].
///
/// An "always" answered before is answered again from [memory] without a
/// dialog. Dismissing the dialog is Don’t allow. A cancelled run takes its
/// dialog down and is answered Don’t allow. Without a navigator (the app
/// not up yet) the answer is Don’t allow. While the dialog is up,
/// [waiting] is told its question (the background run's notification
/// shows it, `BackgroundRuns.needsOk`) and the callback it returns runs
/// once it is answered.
class DialogReadConsent implements AgentReadConsent {
  DialogReadConsent({
    required this.navigator,
    required this.memory,
    this.waiting,
  });

  /// The navigator to show the dialog on.
  final NavigatorState? Function() navigator;
  final ConsentMemory memory;
  final void Function() Function(String question)? waiting;

  @override
  Future<ConsentAnswer> ask({
    required String? reader,
    required String? readerTitle,
    required String source,
    required String sourceTitle,
    Future<void>? cancelled,
  }) async {
    if (reader != null && await memory.allowed(reader, source)) {
      return ConsentAnswer.always;
    }
    final nav = navigator();
    if (nav == null || !nav.mounted) return ConsentAnswer.denied;
    final question = readConsentTitle(sourceTitle);
    final answered = waiting?.call(question);
    final ClaxDialogAnswer got;
    try {
      got = await ClaxDialog.ask(
        nav.context,
        title: question,
        body: readConsentBody(readerTitle),
        suppression: reader == null ? null : 'Always for this app',
        actions: const [
          ClaxDialogAction('Don’t allow'),
          ClaxDialogAction(
            'Allow once',
            kind: ClaxButtonKind.primary,
            suppressedLabel: 'Always allow',
          ),
        ],
        defaultIndex: 1,
        cancelIndex: 0,
        dismissed: cancelled,
      );
    } finally {
      answered?.call();
    }
    if (got.index != 1) return ConsentAnswer.denied;
    if (got.suppressed && reader != null) {
      await memory.allow(reader, source);
      return ConsentAnswer.always;
    }
    return ConsentAnswer.once;
  }
}

/// The consent's title, on one line: "Let Clax read Runs?".
String readConsentTitle(String source) {
  final name = source.trim();
  return 'Let Clax read ${name.isEmpty ? 'another app' : name}?';
}

/// The consent's body, naming the app Clax works on ([app], null for a new
/// one).
String readConsentBody(String? app) {
  final working = switch (app?.trim()) {
    final a? when a.isNotEmpty => a,
    _ => app == null ? 'your new app' : 'this app',
  };
  return 'Clax would use its data in $working. It never changes another '
      'app’s data.';
}
