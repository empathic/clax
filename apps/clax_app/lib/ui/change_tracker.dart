import '../agent/timings.dart';

/// How the last change ended, when it ended other than by landing.
enum ChangeOutcome { replied, failed, stopped, sendFailed }

/// One change in flight: its thread, whether the long-press→landed timing
/// is its own, and the version the worker landed it as.
class _Change {
  _Change({required this.timed});
  bool timed;
  int? version;
}

/// The changes of one app in flight, from the shell's `sent` and the
/// worker's events to the frame that shows them, for the Working chip and
/// the long-press→landed timing.
///
/// Changes are kept per thread. The worker runs one change at a time and
/// each [workerWorking] (naming its threads) is followed by exactly one
/// terminal event for them; the shell's `sent` and the worker's events
/// can come in either order, so a thread the worker already finished is
/// never started again by a late `sent`.
///
/// A change has landed once a frame shows its version: the shell's
/// `landed`, or a `ready` of that version or newer (a page reloaded
/// mid-change opens the latest without `landed`). A frame of a newer
/// version that the worker has not reported yet is taken as the running
/// change's, unless this screen made it by a restore ([restored]).
///
/// The long-press→landed timing runs from Send to that frame
/// (docs/superpowers/specs/2026-10-12-flutter-app-design.md section 6.2).
/// One change is timed at a time: a Send while a timed change is in
/// flight is not timed, so each record is one change's. A change that ends
/// without a frame (a reply, a failure, a cancel, a landing while the view
/// shows an older version) records nothing.
class ChangeTracker {
  ChangeTracker({required this.timings, required this.artifact});

  final Timings timings;

  /// The artifact ID, recorded with each timing.
  final String artifact;

  final _changes = <String, _Change>{};

  /// The threads of the change the worker is running.
  var _running = <String>[];

  /// Threads whose change the worker finished.
  final _finished = <String>{};

  /// Versions this screen made by restoring.
  final _restored = <int>{};

  /// The newest version a frame showed, and whether the view is pinned to
  /// an older one.
  var _shownMax = 0;
  var _pinned = false;

  var _hidden = false;
  ChangeOutcome? _outcome;
  String _outcomeText = '';
  String? _retry;

  /// Whether to show Working: a change is in flight and the person did not
  /// dismiss it.
  bool get working => _changes.isNotEmpty && !_hidden;

  /// How the last change ended, until dismissed or new work starts.
  ChangeOutcome? get outcome => _outcome;
  String get outcomeText => _outcomeText;

  /// The thread whose send failed, for Retry.
  String? get retryThread => _retry;

  /// The shell sent [thread] to the agent.
  void sent(String thread) {
    if (_finished.contains(thread) || _changes.containsKey(thread)) return;
    final timed = !_changes.values.any((c) => c.timed);
    _changes[thread] = _Change(timed: timed);
    if (timed) timings.start(longpressLanded);
    _newWork();
  }

  /// The shell posted [thread] but could not send it.
  void sendFailed(String thread, String text) {
    _changes.remove(thread);
    _end(
      ChangeOutcome.sendFailed,
      text.isEmpty ? 'Couldn’t send the comment' : text,
    );
    _retry = thread;
  }

  void workerWorking(List<String> threads) {
    _running = List.of(threads);
    for (final t in threads) {
      _finished.remove(t);
      _changes.putIfAbsent(t, () => _Change(timed: false));
    }
    _newWork();
  }

  /// The daemon stopped answering; the worker runs the change again.
  void workerInterrupted() => _hidden = false;

  /// The worker landed the running change as [version]. Returns true when
  /// the view shows an older version, so no frame will show this one: the
  /// change ends here and the person should be told it is ready.
  bool workerLanded(int version) {
    final threads = _takeRunning();
    for (final t in threads) {
      _changes[t]?.version = version;
    }
    if (_pinned) {
      for (final t in threads) {
        _changes.remove(t);
      }
      return threads.isNotEmpty;
    }
    _settle();
    return false;
  }

  void workerReplied() =>
      _endRunning(ChangeOutcome.replied, 'Answered in the thread');

  void workerFailed(String reason) =>
      _endRunning(ChangeOutcome.failed, 'Couldn’t make that change: $reason');

  void workerCancelled() => _endRunning(
    ChangeOutcome.stopped,
    'Stopped. It runs again when the app restarts.',
  );

  /// This screen restored a version, which became [version].
  void restored(int version) => _restored.add(version);

  /// The shell's `ready`: a frame shows [version], pinned or following.
  void shown(int version, {required bool pinned}) {
    _pinned = pinned;
    if (!pinned) frame(version);
  }

  /// A frame of the latest version, [version], loaded.
  void frame(int version) {
    final newer = version > _shownMax;
    if (newer) _shownMax = version;
    if (newer && !_restored.contains(version)) {
      // A version the worker has not reported yet: the running change's.
      for (final t in _running) {
        final c = _changes[t];
        if (c != null && c.version == null) _land(c, version);
      }
    }
    _settle();
  }

  /// Hides the chip: the outcome if one shows, else Working until new
  /// work starts.
  void dismiss() {
    if (_outcome != null) {
      _outcome = null;
      _retry = null;
    } else {
      _hidden = true;
    }
  }

  /// Ends every change whose version a frame has shown.
  void _settle() {
    final done = [
      for (final e in _changes.entries)
        if (e.value.version case final v? when v <= _shownMax) e.key,
    ];
    for (final t in done) {
      _land(_changes.remove(t)!, _shownMax);
    }
  }

  void _land(_Change c, int version) {
    if (!c.timed) return;
    c.timed = false;
    timings.stop(
      longpressLanded,
      extra: {'artifact': artifact, 'version': version},
    );
  }

  List<String> _takeRunning() {
    final threads = _running;
    _running = [];
    _finished.addAll(threads);
    return threads;
  }

  void _endRunning(ChangeOutcome outcome, String text) {
    for (final t in _takeRunning()) {
      _changes.remove(t);
    }
    _end(outcome, text);
  }

  void _end(ChangeOutcome outcome, String text) {
    _outcome = outcome;
    _outcomeText = text;
    _retry = null;
  }

  void _newWork() {
    _hidden = false;
    _outcome = null;
    _retry = null;
  }
}
