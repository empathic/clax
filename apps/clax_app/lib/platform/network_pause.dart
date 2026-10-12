import 'dart:async';

import 'package:flutter/widgets.dart' show AppLifecycleState;

import '../agent/backend.dart';
import '../agent/edit_result.dart';
import '../agent/request.dart';

/// Whether the app is in front of the person, and how many times it has
/// left. Android cuts a background app's network (`dumpsys netpolicy`:
/// `blocked=APP_BACKGROUND`) unless a foreground service keeps it.
class AppPresence {
  AppPresence({this._foreground = true});

  bool _foreground;
  int _leaves = 0;
  Completer<void>? _back;

  /// Visible: [AppLifecycleState.resumed] or `inactive` (a dialog, the
  /// notification shade or the app switcher over it).
  bool get foreground => _foreground;

  /// How many times the app went to the background.
  int get leaves => _leaves;

  /// Follows the app's lifecycle.
  void onLifecycle(AppLifecycleState state) {
    final front =
        state == AppLifecycleState.resumed ||
        state == AppLifecycleState.inactive;
    if (front == _foreground) return;
    _foreground = front;
    if (!front) {
      _leaves++;
    } else {
      _back?.complete();
      _back = null;
    }
  }

  /// Completes when the app is in the foreground: at once when it is.
  Future<void> get whenForeground =>
      _foreground ? Future.value() : (_back ??= Completer<void>()).future;
}

/// Whether [e] says the model could not be reached at all, or the
/// connection broke ([AgentErrorKind.network]): what a run sees when
/// Android cut the app's network in the background.
bool isNetworkCut(AgentError e) => e.kind == AgentErrorKind.network;

/// An [AgentBackend] that waits out a network cut by the background: when
/// [inner] fails with [isNetworkCut] and the app left the foreground
/// during that run (or is still out of it), the run pauses ([onPause])
/// until the app is back in the foreground, then runs the request again
/// ([onResume]), at most [maxResumes] times. Any other failure, and a
/// network failure while the app stayed in front, is thrown as before. A
/// cancel while paused throws [AgentError.cancelled].
///
/// Resending here is a transport retry: a network failure means no answer
/// was used, so nothing was applied. A backend that applies work during a
/// run (publishing between model turns) must resume that run instead.
class ResumingBackend implements AgentBackend {
  ResumingBackend(
    this.inner,
    this.presence, {
    this.onPause,
    this.onResume,
    this.maxResumes = 3,
  });

  final AgentBackend inner;
  final AppPresence presence;
  final void Function()? onPause;
  final void Function()? onResume;
  final int maxResumes;

  @override
  String get name => inner.name;

  @override
  Future<EditResult> run(AgentRequest request) async {
    final cancel = CancelWatch(request.cancelled);
    for (var resumes = 0; ; resumes++) {
      final leaves = presence.leaves;
      try {
        return await inner.run(request);
      } on AgentError catch (e) {
        final cutByBackground =
            !presence.foreground || presence.leaves != leaves;
        if (resumes >= maxResumes || !isNetworkCut(e) || !cutByBackground) {
          rethrow;
        }
      }
      cancel.check();
      onPause?.call();
      await Future.any([presence.whenForeground, cancel.whenCancelled]);
      cancel.check();
      onResume?.call();
    }
  }
}
