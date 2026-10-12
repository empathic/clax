import 'dart:async';

import 'package:flutter/material.dart';

import '../agent/backend.dart' show AgentError;

/// How the making screen ended, when it did not end in Cancel (which pops
/// null).
sealed class MakingResult {
  const MakingResult();
}

/// The app was made; [aid] is its artifact ID.
class Made extends MakingResult {
  const Made(this.aid);
  final String aid;
}

/// The person chose to change the request after a failure.
class EditRequest extends MakingResult {
  const EditRequest();
}

/// The request's first five words, as the making screen names it.
String firstWords(String request, [int n = 5]) {
  final words = request.trim().split(RegExp(r'\s+'));
  return words.take(n).join(' ');
}

/// Runs [make] for [request] while showing "Making" with the request's
/// first five words quoted, the seconds elapsed, and Cancel. Pops [Made]
/// with the new artifact's ID.
/// A failure stays on the screen with its reason, the request, Retry and
/// Edit, and a link to Settings when the reason names them. Cancel pops
/// null at once; the abandoned run's result, if it comes, is ignored.
class MakingScreen extends StatefulWidget {
  const MakingScreen({
    super.key,
    required this.request,
    required this.make,
    this.openSettings,
  });

  final String request;
  final Future<String> Function(String request) make;
  final VoidCallback? openSettings;

  @override
  State<MakingScreen> createState() => _MakingScreenState();
}

class _MakingScreenState extends State<MakingScreen> {
  Timer? _tick;
  int _seconds = 0;
  String? _failure;

  /// Bumped by each attempt, so a stale attempt's result is dropped.
  int _attempt = 0;

  @override
  void initState() {
    super.initState();
    _run();
  }

  @override
  void dispose() {
    _tick?.cancel();
    _attempt++;
    super.dispose();
  }

  Future<void> _run() async {
    final attempt = ++_attempt;
    setState(() {
      _failure = null;
      _seconds = 0;
    });
    _tick?.cancel();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() => _seconds++);
    });
    try {
      final aid = await widget.make(widget.request);
      if (!mounted || attempt != _attempt) return;
      _tick?.cancel();
      Navigator.of(context).pop(Made(aid));
    } catch (e) {
      if (!mounted || attempt != _attempt) return;
      _tick?.cancel();
      setState(() => _failure = e is AgentError ? e.message : '$e');
    }
  }

  void _cancel() {
    _attempt++;
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: true,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) _attempt++;
      },
      child: Scaffold(
        body: SafeArea(
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 480),
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: AnimatedSwitcher(
                  duration: const Duration(milliseconds: 200),
                  child: _failure == null ? _making(context) : _failed(context),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _making(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return Column(
      key: const ValueKey('making'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Icon(Icons.auto_awesome_rounded, size: 40, color: scheme.primary),
        const SizedBox(height: 24),
        Text(
          'Making “${firstWords(widget.request)}”…',
          textAlign: TextAlign.center,
          style: theme.textTheme.headlineSmall,
        ),
        const SizedBox(height: 24),
        ClipRRect(
          borderRadius: BorderRadius.circular(4),
          child: const LinearProgressIndicator(minHeight: 6),
        ),
        const SizedBox(height: 12),
        Text(
          '$_seconds s',
          textAlign: TextAlign.center,
          style: theme.textTheme.bodyMedium?.copyWith(
            color: scheme.onSurfaceVariant,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
        const SizedBox(height: 32),
        Center(
          child: OutlinedButton(
            onPressed: _cancel,
            child: const Text('Cancel'),
          ),
        ),
      ],
    );
  }

  Widget _failed(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final reason = _failure!;
    final settings = widget.openSettings != null && reason.contains('Settings');
    return Column(
      key: const ValueKey('failed'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Icon(Icons.error_outline_rounded, size: 40, color: scheme.error),
        const SizedBox(height: 16),
        Text(
          "Couldn't make it",
          textAlign: TextAlign.center,
          style: theme.textTheme.headlineSmall,
        ),
        const SizedBox(height: 8),
        Text(
          reason,
          textAlign: TextAlign.center,
          style: theme.textTheme.bodyLarge?.copyWith(
            color: scheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: 24),
        DecoratedBox(
          decoration: BoxDecoration(
            color: scheme.surfaceContainerHighest,
            borderRadius: BorderRadius.circular(16),
          ),
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Text(widget.request, style: theme.textTheme.bodyLarge),
          ),
        ),
        const SizedBox(height: 24),
        Wrap(
          alignment: WrapAlignment.center,
          spacing: 12,
          runSpacing: 12,
          children: [
            if (settings)
              FilledButton(
                onPressed: () {
                  _attempt++;
                  Navigator.of(context).pop();
                  widget.openSettings!();
                },
                child: const Text('Open Settings'),
              ),
            settings
                ? OutlinedButton(onPressed: _run, child: const Text('Retry'))
                : FilledButton(onPressed: _run, child: const Text('Retry')),
            TextButton(
              onPressed: () => Navigator.of(context).pop(const EditRequest()),
              child: const Text('Edit'),
            ),
            TextButton(onPressed: _cancel, child: const Text('Cancel')),
          ],
        ),
      ],
    );
  }
}
