import 'dart:async';

import 'package:clax_app/platform/clock.dart';

/// A clock for the app's own deadlines in an integration test: [now] is
/// the wall clock's, and a delay ends only when the test [release]s it.
/// The app's inspector runs on one there, so a loaded machine's slow page
/// load is waited for rather than cut off at the inspector's limit; the
/// rig's step guard (`_cap`) ends a load that never comes. The limits
/// themselves are tested on a step clock (test/ui/inspector_surface_test.dart).
class HeldClock implements Clock {
  final _held = <Completer<void>>[];

  @override
  DateTime now() => DateTime.now();

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _held.add(c);
    return c.future;
  }

  /// Ends every delay asked for so far.
  void release() {
    for (final c in _held) {
      if (!c.isCompleted) c.complete();
    }
    _held.clear();
  }
}
