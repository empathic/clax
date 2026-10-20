import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../core/core_client.dart';
import 'kit/kit.dart';
import 'log_details.dart';

/// Clax could not start: "Clax couldn’t start", centred, with **Try
/// again** where there is an [onRestart] and Copy details, which puts
/// [reason] and [tail] on the clipboard for a report. [tail] (the last
/// lines worth reporting, oldest first) sits closed under Details;
/// [reason] is never on screen.
class ErrorScreen extends StatefulWidget {
  const ErrorScreen({
    super.key,
    required this.reason,
    required this.tail,
    this.onRestart,
  });

  /// The core did not open ([failure]): nothing in this launch can try
  /// again, so the screen says to open Clax again. A failed migration says
  /// its data is kept, and its details name the backup made before it.
  ErrorScreen.failed(CoreFailed failure, {Key? key})
    : this(
        key: key,
        reason: '${failure.code}: ${failure.message}',
        tail: [
          failure.message,
          if (failure.backup case final b?) 'The backup made before it: $b',
        ],
      );

  final String reason;
  final List<String> tail;
  final VoidCallback? onRestart;

  @override
  State<ErrorScreen> createState() => _ErrorScreenState();
}

class _ErrorScreenState extends State<ErrorScreen> {
  var _copied = false;

  @override
  void initState() {
    super.initState();
    debugPrint('clax: ${widget.reason}');
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
        body: widget.onRestart == null
            ? 'It stopped while starting. Quit Clax and open it again, or '
                  'copy the details for a report.'
            : 'It stopped while starting. Try again, or copy the details for '
                  'a report.',
        actions: [
          if (widget.onRestart case final restart?)
            ClaxButton(
              label: 'Try again',
              kind: ClaxButtonKind.primary,
              isDefault: true,
              onPressed: restart,
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
