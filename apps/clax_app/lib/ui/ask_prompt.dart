import 'package:flutter/widgets.dart';

import '../daemon/clock.dart';
import 'host_protocol.dart';
import 'kit/button.dart';
import 'kit/dialog.dart';

/// The shell's permission prompt ([AskMsg]), drawn by the app when it
/// declares `ask` native.
abstract final class AskPrompt {
  /// The line under Allow until it arms.
  static const waiting = 'Allow in a moment…';

  /// Shows [m] as a [ClaxDialog] and completes with its answer.
  ///
  /// - [AskMsg.deny] has focus on the first frame, and answers
  ///   [AskAnswerValue.deny].
  /// - [AskMsg.allow] is the default action, disabled for [AskMsg.armMs] on
  ///   [clock] from the first frame, with [waiting] under it until then.
  ///   It answers [AskAnswerValue.allow] only for a tap or click whose
  ///   press began after it armed, or a Return (or a Space on it) whose key
  ///   went down after.
  /// - With [AskMsg.always], a checkbox of that label sits above the
  ///   buttons; Allow with it checked answers [AskAnswerValue.always].
  /// - Escape, a tap outside and Back answer [AskAnswerValue.dismiss],
  ///   never [AskAnswerValue.deny].
  /// - When [cancelled] completes first, the prompt is taken down and
  ///   answers [AskAnswerValue.dismiss]; the caller sends no answer for it.
  static Future<AskAnswerValue> show(
    BuildContext context,
    AskMsg m, {
    Clock clock = const SystemClock(),
    Future<void>? cancelled,
  }) async {
    final a = await ClaxDialog.ask(
      context,
      title: m.title,
      body: m.body.isEmpty ? null : m.body,
      suppression: m.always,
      actions: [
        ClaxDialogAction(m.deny),
        ClaxDialogAction(m.allow, kind: ClaxButtonKind.primary),
      ],
      defaultIndex: 1,
      focusIndex: 0,
      arming: ClaxArming(
        delay: Duration(milliseconds: m.armMs),
        waiting: waiting,
        clock: clock,
      ),
      dismissed: cancelled,
    );
    return switch (a.index) {
      0 => AskAnswerValue.deny,
      1 => a.suppressed ? AskAnswerValue.always : AskAnswerValue.allow,
      _ => AskAnswerValue.dismiss,
    };
  }
}
