/// The owner's consent for the agent to read another app's documents,
/// asked in a dialog and remembered per pair of apps when the owner says
/// so.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

import '../agent/read_consent.dart';

/// The pairs of apps whose reads the owner allowed always: the agent
/// working on `reader` may read `source`'s documents.
abstract interface class ConsentMemory {
  Future<bool> allowed(String reader, String source);
  Future<void> allow(String reader, String source);
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

/// [AgentReadConsent] in a dialog over the app: "Let the agent read
/// “Runs” while it works on “Dashboard”?" with Allow once, Always for this app
/// (not for a new app, which has no ID to remember it by) and Don't allow.
///
/// An "always" answered before is answered again from [memory] without a
/// dialog. Dismissing the dialog is Don't allow. A cancelled run takes its
/// dialog down and is answered Don't allow. Without a navigator (the app
/// not up yet) the answer is Don't allow.
class DialogReadConsent implements AgentReadConsent {
  DialogReadConsent({required this.navigator, required this.memory});

  /// The navigator to show the dialog on.
  final NavigatorState? Function() navigator;
  final ConsentMemory memory;

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
    final answer = Completer<ConsentAnswer>();
    final route = DialogRoute<ConsentAnswer>(
      context: nav.context,
      builder: (context) => ReadConsentDialog(
        source: sourceTitle,
        app: readerTitle,
        canRemember: reader != null,
      ),
    );
    unawaited(
      nav.push(route).then((a) {
        if (!answer.isCompleted) answer.complete(a ?? ConsentAnswer.denied);
      }),
    );
    unawaited(
      cancelled?.then((_) {
        if (answer.isCompleted) return;
        answer.complete(ConsentAnswer.denied);
        if (route.isActive) route.navigator?.removeRoute(route);
      }),
    );
    final a = await answer.future;
    if (a == ConsentAnswer.always && reader != null) {
      await memory.allow(reader, source);
    }
    return a;
  }
}

/// The question itself; pops with the [ConsentAnswer] chosen.
class ReadConsentDialog extends StatelessWidget {
  const ReadConsentDialog({
    super.key,
    required this.source,
    required this.app,
    required this.canRemember,
  });

  /// The title of the app whose documents the agent would read.
  final String source;

  /// The title of the app the agent works on; null for a new app.
  final String? app;

  /// Whether to offer Always for this app.
  final bool canRemember;

  static String _name(String title) =>
      title.trim().isEmpty ? 'another app' : '“${title.trim()}”';

  @override
  Widget build(BuildContext context) {
    final working = switch (app) {
      final a? when a.trim().isNotEmpty => _name(a),
      _ => app == null ? 'your new app' : 'this app',
    };
    void answer(ConsentAnswer a) => Navigator.of(context).pop(a);
    return AlertDialog(
      title: Text(
        'Let the agent read ${_name(source)} while it works on '
        '$working?',
      ),
      content: const Text(
        'It reads that app’s data to use in this one. It never changes '
        'another app’s data.',
      ),
      actions: [
        TextButton(
          onPressed: () => answer(ConsentAnswer.denied),
          child: const Text('Don’t allow'),
        ),
        if (canRemember)
          TextButton(
            onPressed: () => answer(ConsentAnswer.always),
            child: const Text('Always for this app'),
          ),
        FilledButton(
          onPressed: () => answer(ConsentAnswer.once),
          child: const Text('Allow once'),
        ),
      ],
    );
  }
}
