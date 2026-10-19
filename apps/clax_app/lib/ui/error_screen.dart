import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'kit/kit.dart';
import 'log_details.dart';

/// The embedded daemon could not be kept running: "Clax couldn’t start",
/// centred, with **Try again** ([onRestart], which starts it afresh) and
/// Copy details, which puts [reason] and [tail] on the clipboard for a
/// report. [tail] (the daemon's last lines, its log and standard error,
/// oldest first) sits closed under Details; [reason] is never on screen.
class ErrorScreen extends StatefulWidget {
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
  State<ErrorScreen> createState() => _ErrorScreenState();
}

class _ErrorScreenState extends State<ErrorScreen> {
  var _copied = false;

  @override
  void initState() {
    super.initState();
    debugPrint('clax daemon: ${widget.reason}');
  }

  String get _log =>
      widget.tail.isEmpty ? 'Clax wrote nothing.' : widget.tail.join('\n');

  Future<void> _copy() async {
    await Clipboard.setData(
      ClipboardData(text: '${widget.reason}\n\n$_log'),
    );
    if (mounted) setState(() => _copied = true);
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    body: SafeArea(
      child: ClaxEmpty(
        icon: Icons.error_outline_rounded,
        title: 'Clax couldn’t start',
        body:
            'It stopped while starting. Try again, or copy the details for '
            'a report.',
        actions: [
          ClaxButton(
            label: 'Try again',
            kind: ClaxButtonKind.primary,
            isDefault: true,
            onPressed: widget.onRestart,
          ),
          ClaxButton(
            key: const ValueKey('copy-details'),
            label: _copied ? 'Copied' : 'Copy details',
            icon: _copied ? Icons.check_rounded : Icons.copy_rounded,
            onPressed: () => _copy(),
          ),
        ],
        extra: LogDetails(text: _log),
      ),
    ),
  );
}
