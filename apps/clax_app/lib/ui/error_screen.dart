import 'package:flutter/material.dart';

/// The embedded daemon could not be kept running: why ([reason]), the last
/// lines it wrote ([tail], its log and standard error, oldest first), and
/// Restart, which starts it afresh.
class ErrorScreen extends StatelessWidget {
  const ErrorScreen({
    super.key,
    required this.reason,
    required this.tail,
    required this.onRestart,
  });

  final String reason;
  final List<String> tail;
  final VoidCallback onRestart;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.fromLTRB(24, 48, 24, 32),
          children: [
            Icon(Icons.error_outline_rounded, size: 48, color: scheme.error),
            const SizedBox(height: 16),
            Text("Clax couldn't start", style: theme.textTheme.headlineSmall),
            const SizedBox(height: 8),
            Text(reason, style: theme.textTheme.bodyLarge),
            const SizedBox(height: 24),
            Align(
              alignment: AlignmentDirectional.centerStart,
              child: FilledButton.icon(
                onPressed: onRestart,
                icon: const Icon(Icons.restart_alt_rounded),
                label: const Text('Restart'),
              ),
            ),
            const SizedBox(height: 32),
            Text(
              'The last lines clax wrote',
              style: theme.textTheme.titleSmall?.copyWith(
                color: scheme.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: 8),
            Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: scheme.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(12),
              ),
              child: SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                child: Text(
                  tail.isEmpty ? 'clax wrote nothing.' : tail.join('\n'),
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: 'monospace',
                    fontFamilyFallback: const ['Menlo', 'Courier'],
                    color: scheme.onSurface,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
