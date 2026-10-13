import 'dart:typed_data';

import '../agent/progress.dart';
import '../agent/timings.dart';

/// Send in the composer to the first step of that change the Working chip
/// shows (docs/superpowers/specs/2026-10-12-flutter-app-round2-design.md
/// R§6.5).
const sendFirstStep = 'send-first-step';

/// The last render of the running change: its screenshot and the number of
/// uncaught errors it logged.
typedef Inspection = ({Uint8List png, int errors});

/// How the last change ended, when it ended other than by landing.
enum ChangeOutcome { replied, failed, stopped, sendFailed }

/// One change in flight: its thread, whether the long-press→landed timing
/// is its own, and the version the worker landed it as.
class _Change {
  _Change({required this.timed});
  bool timed;

  /// Whether the send→first-step timing is this change's and still runs.
  var stepTimed = false;
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
/// A change has landed once a frame shows the version the worker reported
/// for it: the shell's `landed`, or a `ready` of that version or newer (a
/// page reloaded mid-change opens the latest without `landed`). A frame of
/// a version the worker has not reported is never credited to a change: it
/// may be a restore's or another client's publish. When the frame comes
/// first, the change lands (and its timing stops) on the worker's report.
///
/// The long-press→landed timing runs from Send to that frame
/// (docs/superpowers/specs/2026-10-12-flutter-app-design.md section 6.2).
/// One change is timed at a time: a Send while a timed change is in
/// flight is not timed, so each record is one change's. A Send the worker
/// already started is timed from the Send. A change that ends
/// without a frame (a reply, a failure, a cancel, a landing while the view
/// shows an older version) records nothing.
///
/// Narration (R§9.6): the running change's steps come from the worker's
/// [WorkerProgress] events whose threads are exactly the running change's
/// ([workerProgress]); a [Narration] or [ToolStarted] is a step, a
/// resent [AttemptStarted] the step "Trying again", an [Inspected] the
/// [inspection]. Steps start afresh with each run and stop when it ends.
/// The send→first-step timing ([sendFirstStep]) runs with the timed
/// change, from Send to its first step; a step the worker reported before
/// the Send, or a change that ends without one, records nothing.
///
/// Retry (R§9.5): a failed change keeps its threads ([failedThreads]) until
/// the chip is dismissed or new work starts. [retryStarted], then
/// [retried] (the worker queued it: the change shows running again) or
/// [retryRefused].
class ChangeTracker {
  ChangeTracker({
    required this.timings,
    required this.artifact,
    DateTime Function()? now,
  }) : _now = now ?? DateTime.now;

  final Timings timings;
  final DateTime Function() _now;

  /// The artifact ID, recorded with each timing.
  final String artifact;

  final _changes = <String, _Change>{};

  /// The threads of the change the worker is running.
  var _running = <String>[];

  /// Threads whose change the worker finished.
  final _finished = <String>{};

  /// The newest version a frame showed, and whether the view is pinned to
  /// an older one.
  var _shownMax = 0;
  var _pinned = false;

  var _hidden = false;
  ChangeOutcome? _outcome;
  String _outcomeText = '';
  String? _retry;

  // The running change's narration.
  final _steps = <String>[];
  var _stepCount = 0;
  Inspection? _inspection;
  DateTime? _since;
  var _landing = false;
  var _again = false;

  // Retry of a failed change.
  var _failed = const <String>[];
  var _retrying = false;

  /// How many steps [steps] keeps.
  static const shownSteps = 3;

  /// The latest step of the running change, or null before its first.
  String? get step => _steps.isEmpty ? null : _steps.last;

  /// The running change's last [shownSteps] steps, oldest first.
  List<String> get steps => List.unmodifiable(_steps);

  /// How many steps the running change has taken.
  int get stepCount => _stepCount;

  /// The running change's last render, if it rendered.
  Inspection? get inspection => _inspection;

  /// When the work in flight began: the first Send or start since nothing
  /// was in flight; null when nothing is.
  DateTime? get since => _since;

  /// Whether the worker landed the running change and a frame of it is
  /// awaited.
  bool get landing => _landing;

  /// Whether a Retry queued the change again and the worker has not
  /// started it yet.
  bool get again => _again;

  /// The threads of the failed change, for Retry; empty when there is
  /// nothing to retry.
  List<String> get failedThreads => _failed;

  /// Whether a Retry is being queued.
  bool get retrying => _retrying;

  /// The threads of the change the worker is running; empty when none.
  List<String> get running => List.unmodifiable(_running);

  /// The threads of the work in flight: the running change's, else those
  /// sent and not yet started; empty when nothing is in flight.
  List<String> get threads =>
      _running.isNotEmpty ? running : List.unmodifiable(_changes.keys);

  /// The version the last change landed as, until new work starts; null
  /// when the last change did not land.
  int? get landed => _landed;
  int? _landed;

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
    if (_finished.contains(thread)) return;
    final timed = !_changes.values.any((c) => c.timed);
    _begin();
    // The worker may have started it already (its poll wakes before the
    // shell hears the send's answer): time it from here.
    final c = _changes.putIfAbsent(thread, () => _Change(timed: false));
    if (timed && c.version == null) {
      c.timed = true;
      timings.start(longpressLanded);
      if (!_running.contains(thread) || _stepCount == 0) {
        c.stepTimed = true;
        timings.start(sendFirstStep);
      }
    }
    _newWork();
  }

  /// The shell posted [thread] but could not send it.
  void sendFailed(String thread, String text) {
    _changes.remove(thread);
    _idle();
    _end(
      ChangeOutcome.sendFailed,
      text.isEmpty ? 'Couldn’t send the comment' : text,
    );
    _retry = thread;
  }

  void workerWorking(List<String> threads) {
    _begin();
    _running = List.of(threads);
    _clearRun();
    _again = false;
    for (final t in threads) {
      _finished.remove(t);
      _changes.putIfAbsent(t, () => _Change(timed: false));
    }
    _newWork();
  }

  /// The daemon stopped answering; the worker runs the change again.
  void workerInterrupted() {
    _hidden = false;
    _addStep('Waiting for Clax');
  }

  /// A progress event of the worker's change for [threads]. Answers
  /// whether what the chip shows changed: false for another change's
  /// event, or one that is not a step or a render.
  bool workerProgress(List<String> threads, AgentProgress p) {
    if (_running.isEmpty ||
        threads.length != _running.length ||
        !threads.every(_running.contains)) {
      return false;
    }
    switch (p) {
      case Narration(:final text) || ToolStarted(label: final text):
        if (!_addStep(text)) return false;
        for (final t in _running) {
          final c = _changes[t];
          if (c == null || !c.stepTimed) continue;
          c.stepTimed = false;
          timings.stop(sendFirstStep, extra: {'artifact': artifact});
        }
        return true;
      case AttemptStarted(:final attempt) when attempt > 1:
        return _addStep('Trying again');
      case Inspected(:final png, :final errors):
        _inspection = (png: png, errors: errors);
        return true;
      default:
        return false;
    }
  }

  /// A Retry of [failedThreads] is being queued.
  void retryStarted() => _retrying = true;

  /// The worker queued the Retry: the change shows running again. Does
  /// nothing once new work (the worker starting it, a Send) or a dismissal
  /// ended the Retry.
  void retried() {
    final threads = _failed;
    if (!_retrying || threads.isEmpty) return;
    _retrying = false;
    _begin();
    for (final t in threads) {
      _finished.remove(t);
      _changes.putIfAbsent(t, () => _Change(timed: false));
    }
    _newWork();
    _again = true;
  }

  /// The worker had nothing to run again (the thread was answered or
  /// written on since, or the worker is not running). Does nothing once
  /// new work or a dismissal ended the Retry.
  void retryRefused() {
    if (!_retrying) return;
    _retrying = false;
    _failed = const [];
    _outcomeText = 'Nothing to run again: reply in the thread.';
  }

  /// The worker landed the running change as [version]. Returns true when
  /// the view shows an older version, so no frame will show this one: the
  /// change ends here and the person should be told it is ready.
  bool workerLanded(int version) {
    final threads = _takeRunning();
    for (final t in threads) {
      _changes[t]?.version = version;
    }
    if (threads.isNotEmpty) _landed = version;
    if (_pinned) {
      for (final t in threads) {
        _changes.remove(t);
      }
      _idle();
      return threads.isNotEmpty;
    }
    _landing = threads.isNotEmpty;
    _settle();
    return false;
  }

  void workerReplied() =>
      _endRunning(ChangeOutcome.replied, 'Answered in the thread');

  void workerFailed(String reason) {
    final threads = List.of(_running);
    _endRunning(ChangeOutcome.failed, 'Couldn’t make that change: $reason');
    _failed = threads;
  }

  void workerCancelled() {
    // The person's Stop already ended it ([stopped]).
    if (_running.isEmpty && _outcome == ChangeOutcome.stopped) return;
    _endRunning(
      ChangeOutcome.stopped,
      'Stopped. It runs again when the app restarts.',
    );
  }

  /// The person stopped the work in flight: every change ends, and the
  /// worker's [workerCancelled] that follows changes nothing. Its threads
  /// are kept for Retry ([failedThreads]), as a failure's are.
  void stopped() {
    final kept = List.of(threads);
    _takeRunning();
    _changes.clear();
    _idle();
    _end(
      ChangeOutcome.stopped,
      'Stopped. It waits until you run it again or add to the comment.',
    );
    _failed = kept;
  }

  /// The shell's `ready`: a frame shows [version], pinned or following.
  void shown(int version, {required bool pinned}) {
    _pinned = pinned;
    if (!pinned) frame(version);
  }

  /// A frame of the latest version, [version], loaded.
  void frame(int version) {
    if (version > _shownMax) _shownMax = version;
    _settle();
  }

  /// Hides the chip: the outcome if one shows, else Working until new
  /// work starts.
  void dismiss() {
    if (_outcome != null) {
      _outcome = null;
      _retry = null;
      _failed = const [];
      _retrying = false;
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
    if (done.isNotEmpty) _landing = false;
    _idle();
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
    _clearRun();
    return threads;
  }

  void _endRunning(ChangeOutcome outcome, String text) {
    for (final t in _takeRunning()) {
      _changes.remove(t);
    }
    _idle();
    _end(outcome, text);
  }

  void _end(ChangeOutcome outcome, String text) {
    _outcome = outcome;
    _outcomeText = text;
    _retry = null;
    _failed = const [];
    _retrying = false;
  }

  void _newWork() {
    _hidden = false;
    _landed = null;
    _outcome = null;
    _retry = null;
    _failed = const [];
    _retrying = false;
  }

  /// Work comes in: [since] starts unless work is already in flight.
  void _begin() {
    if (_changes.isEmpty) _since ??= _now();
  }

  /// With nothing left in flight, the chip's running state is over.
  void _idle() {
    if (_changes.isNotEmpty) return;
    _since = null;
    _landing = false;
    _again = false;
  }

  /// Drops the steps and render of the run that ended.
  void _clearRun() {
    _steps.clear();
    _stepCount = 0;
    _inspection = null;
  }

  /// Adds [text] as the latest step unless it repeats the latest; answers
  /// whether it did.
  bool _addStep(String text) {
    if (text.isEmpty || (_steps.isNotEmpty && _steps.last == text)) {
      return false;
    }
    _steps.add(text);
    if (_steps.length > shownSteps) _steps.removeAt(0);
    _stepCount++;
    return true;
  }
}
