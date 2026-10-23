/// Get my apps: the apps of the account this device is signed in to on a
/// hosted Clax, to bring down (spec 2026-10-21-app-push-pull §5.5).
library;

import 'package:flutter/material.dart';

import '../../clax/api.dart' show ClaxApiError;
import '../../remote/models.dart';
import '../../remote/puller.dart';
import '../../remote/remote_api.dart';
import '../../remote/remote_errors.dart';
import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';

/// Lists the apps the account signed in to [rid] owns or holds a share
/// on (those with a bundle: an app published from the web alone is not
/// one to get), with what it may do with each. The person picks which to
/// bring down; **Get** brings each through [pull], one after another, and
/// says how each went. Apps come without their data, and the list says so.
class LibraryScreen extends StatefulWidget {
  const LibraryScreen({
    super.key,
    required this.remote,
    required this.rid,
    required this.pull,
    this.host = 'clax.dev',
  });

  final RemoteApi remote;
  final String rid;
  final Future<PullOutcome> Function(RemoteApp app) pull;

  /// The remote's name, for the title.
  final String host;

  @override
  State<LibraryScreen> createState() => _LibraryScreenState();
}

class _LibraryScreenState extends State<LibraryScreen> {
  List<RemoteApp>? _apps;
  String? _error;
  final _picked = <String>{};
  final _done = <String, PullOutcome>{};
  var _getting = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    setState(() {
      _error = null;
      _apps = null;
    });
    try {
      final apps = await widget.remote.apps(widget.rid);
      if (mounted) setState(() => _apps = apps);
    } on ClaxApiError catch (e) {
      if (mounted) {
        setState(
          () => _error = remoteFailureLine(
            e,
            host: widget.host,
            action: 'list your apps',
            otherwise: 'Couldn’t read your apps on ${widget.host}.',
          ),
        );
      }
    }
  }

  Future<void> _get() async {
    final apps = [
      for (final a in _apps ?? const <RemoteApp>[])
        if (_picked.contains(a.artifactId)) a,
    ];
    setState(() => _getting = true);
    for (final a in apps) {
      PullOutcome out;
      try {
        out = await widget.pull(a);
      } on Object catch (e) {
        out = PullRefused('Couldn’t get ${a.title}.');
        debugPrint('clax remote: the pull of ${a.artifactId} failed: $e');
      }
      if (!mounted) return;
      setState(() {
        _done[a.artifactId] = out;
        _picked.remove(a.artifactId);
      });
    }
    if (mounted) setState(() => _getting = false);
  }

  static String _access(RemoteApp a) => switch (a.access) {
    'owner' => 'Yours · v${a.bundleVersion}',
    'editor' => 'Shared with you · can edit · v${a.bundleVersion}',
    'commenter' => 'Shared with you · can comment · v${a.bundleVersion}',
    _ => 'Shared with you · can view · v${a.bundleVersion}',
  };

  static String? _outcome(PullOutcome? o) => switch (o) {
    null => null,
    PullInstalled() => 'Got it',
    PullHave() => 'Already here',
    PullUpdate() => 'Already here; an update is waiting',
    PullCancelled() => null,
    PullRefused(:final message) => message,
  };

  @override
  Widget build(BuildContext context) {
    final apps = _apps;
    final muted = claxPalette(context).muted;
    final theme = Theme.of(context);
    final count = _picked.length;
    final Widget body;
    if (apps == null && _error != null) {
      body = ClaxEmpty(
        icon: Icons.cloud_off_rounded,
        title: _error!,
        actions: [ClaxButton(label: 'Try again', onPressed: _load)],
      );
    } else if (apps == null) {
      body = const Center(child: CircularProgressIndicator());
    } else if (apps.isEmpty) {
      body = ClaxEmpty(
        icon: Icons.cloud_outlined,
        title: 'No apps on ${widget.host} yet',
        body: 'Apps you publish, and apps people share with you, show here.',
      );
    } else {
      body = ListView(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 32),
        children: [
          Padding(
            padding: const EdgeInsets.only(bottom: ClaxSpace.s8),
            child: Text(
              'Apps come without their data.',
              style: theme.textTheme.bodyMedium?.copyWith(color: muted),
            ),
          ),
          for (final a in apps) ...[
            ClaxCheckbox(
              key: ValueKey('pick-${a.artifactId}'),
              value: _picked.contains(a.artifactId),
              label: a.title,
              detail: _access(a),
              onChanged: _getting || _done[a.artifactId] is PullInstalled
                  ? null
                  : (v) => setState(
                      () => v
                          ? _picked.add(a.artifactId)
                          : _picked.remove(a.artifactId),
                    ),
            ),
            if (_outcome(_done[a.artifactId]) case final said?)
              Padding(
                padding: const EdgeInsets.only(left: 40, bottom: 8),
                child: Text(
                  said,
                  key: ValueKey('pulled-${a.artifactId}'),
                  style: theme.textTheme.bodySmall?.copyWith(color: muted),
                ),
              ),
          ],
          const SizedBox(height: ClaxSpace.s16),
          Align(
            alignment: Alignment.centerRight,
            child: ClaxButton(
              key: const ValueKey('library-get'),
              kind: ClaxButtonKind.primary,
              isDefault: true,
              label: count > 1 ? 'Get $count apps' : 'Get',
              onPressed: count == 0 || _getting ? null : _get,
            ),
          ),
        ],
      );
    }
    return ClaxScaffold(title: 'Your apps on ${widget.host}', body: body);
  }
}
