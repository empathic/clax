import 'dart:typed_data';

import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/ui/change_tracker.dart';
import 'package:flutter_test/flutter_test.dart';

/// Logs the long-press→landed timing in [log] and the send→first-step
/// timing in [stepLog].
class _Timings implements Timings {
  final log = <String>[];
  final stepLog = <String>[];
  @override
  void start(String name) =>
      name == sendFirstStep ? stepLog.add('start') : log.add('start');
  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) {
    if (name == sendFirstStep) {
      stepLog.add('stop ${extra['artifact']}');
    } else {
      log.add('stop v${extra['version']}');
    }
    return null;
  }

  @override
  List<TimingRecord> recent(int n) => const [];
}

void main() {
  late _Timings timings;
  late ChangeTracker t;

  late DateTime now;

  setUp(() {
    timings = _Timings();
    now = DateTime.utc(2026, 10, 12, 9);
    t = ChangeTracker(timings: timings, artifact: 'a1', now: () => now)
      ..shown(3, pinned: false);
  });

  test('Send, working, the worker lands, then the frame: timed from Send', () {
    t.sent('t1');
    expect(timings.log, ['start']);
    expect(t.working, isTrue);
    t.workerWorking(['t1']);
    expect(t.workerLanded(4), isFalse);
    expect(t.working, isTrue, reason: 'until the frame shows it');
    t.frame(4);
    expect(t.working, isFalse);
    expect(t.outcome, isNull);
    expect(timings.log, ['start', 'stop v4']);
  });

  test(
    'a frame before the worker reports its version waits for the report',
    () {
      t.sent('t1');
      t.workerWorking(['t1']);
      t.frame(4);
      expect(timings.log, [
        'start',
      ], reason: 'v4 is not known to be this change');
      expect(t.working, isTrue);
      t.workerLanded(4);
      expect(t.working, isFalse);
      expect(timings.log, ['start', 'stop v4']);
    },
  );

  test('the worker starting before the shell reports Send is still timed', () {
    t.workerWorking(['t1']);
    t.sent('t1');
    expect(timings.log, ['start']);
    t.workerLanded(4);
    t.frame(4);
    expect(t.working, isFalse);
    expect(timings.log, ['start', 'stop v4']);
  });

  test('Send then the worker start, landing then frame, is timed once', () {
    t.sent('t1');
    t.workerWorking(['t1']);
    t.workerLanded(4);
    t.frame(4);
    expect(timings.log, ['start', 'stop v4']);
  });

  test('a late Send is not timed while another change is', () {
    t.sent('t1');
    t.workerWorking(['t2']);
    t.sent('t2');
    expect(timings.log, ['start']);
  });

  test("another client's version does not end the running change's timing", () {
    t.sent('t1');
    t.workerWorking(['t1']);
    t.frame(4);
    expect(timings.log, ['start']);
    t.workerLanded(5);
    expect(t.working, isTrue);
    t.frame(5);
    expect(t.working, isFalse);
    expect(timings.log, ['start', 'stop v5']);
  });

  test('a frame of the same version (another page) is not a landing', () {
    t.sent('t1');
    t.workerWorking(['t1']);
    t.frame(3);
    expect(timings.log, ['start']);
    expect(t.working, isTrue);
  });

  test('a failure before the shell reports Send still ends the change', () {
    t.workerWorking(['t1']);
    t.workerFailed('The API key was refused');
    t.sent('t1');
    expect(t.working, isFalse);
    expect(t.outcome, ChangeOutcome.failed);
    expect(t.outcomeText, contains('The API key was refused'));
    expect(timings.log, isEmpty);
  });

  test('a reply ends the change and drops its timing', () {
    t.sent('t1');
    t.workerWorking(['t1']);
    t.workerReplied();
    expect(t.working, isFalse);
    expect(t.outcome, ChangeOutcome.replied);
    t.frame(4);
    expect(timings.log, ['start'], reason: 'no record for a reply');
  });

  test('a cancel shows as stopped, not failed', () {
    t.sent('t1');
    t.workerWorking(['t1']);
    t.workerCancelled();
    expect(t.outcome, ChangeOutcome.stopped);
    expect(t.working, isFalse);
  });

  test('an interrupted change keeps working', () {
    t.sent('t1');
    t.workerWorking(['t1']);
    t.workerInterrupted();
    expect(t.working, isTrue);
  });

  test('a page reloaded mid-change ends it on the newer version', () {
    t.sent('t1');
    t.workerWorking(['t1']);
    t.workerLanded(4);
    // The reloaded shell opens on the latest without a `landed`.
    t.shown(4, pinned: false);
    expect(t.working, isFalse);
    expect(timings.log, ['start', 'stop v4']);
  });

  test('a landing while the view shows an older version ends the change', () {
    t.shown(2, pinned: true);
    t.sent('t1');
    t.workerWorking(['t1']);
    expect(t.workerLanded(4), isTrue, reason: 'say the version is ready');
    expect(t.working, isFalse);
    expect(timings.log, ['start'], reason: 'no frame, no record');
  });

  test("a Restore's frame does not end another change or its timing", () {
    t.sent('t1');
    t.workerWorking(['t1']);
    // The shell may follow the restored version before its response.
    t.frame(4);
    expect(t.working, isTrue);
    expect(timings.log, ['start']);
    t.workerLanded(5);
    t.frame(5);
    expect(t.working, isFalse);
    expect(timings.log, ['start', 'stop v5']);
  });

  test('only one change is timed at a time; each record is its own', () {
    t.sent('t1');
    t.sent('t2');
    expect(timings.log, ['start']);
    t.workerWorking(['t1']);
    t.workerLanded(4);
    t.frame(4);
    expect(t.working, isTrue, reason: 't2 is still to run');
    t.workerWorking(['t2']);
    t.workerLanded(5);
    t.frame(5);
    expect(t.working, isFalse);
    expect(timings.log, ['start', 'stop v4']);
  });

  test('a failed send ends that change and offers Retry', () {
    t.sent('t1');
    t.sendFailed('t1', 'Offline');
    expect(t.working, isFalse);
    expect(t.outcome, ChangeOutcome.sendFailed);
    expect(t.retryThread, 't1');
    t.sent('t1');
    expect(t.outcome, isNull);
    expect(t.working, isTrue);
  });

  test('every chip can be dismissed; new work shows again', () {
    t.sent('t1');
    t.dismiss();
    expect(t.working, isFalse);
    t.workerWorking(['t1']);
    expect(t.working, isTrue);
    t.workerFailed('x');
    t.dismiss();
    expect(t.outcome, isNull);
  });

  test('a worker change this screen did not send still shows Working', () {
    t.workerWorking(['recovered']);
    expect(t.working, isTrue);
    t.workerLanded(4);
    t.frame(4);
    expect(t.working, isFalse);
    expect(timings.log, isEmpty);
  });

  group('narration', () {
    test('steps of the running change, latest last, three at most', () {
      t.sent('t1');
      t.workerWorking(['t1']);
      expect(t.step, isNull);
      expect(
        t.workerProgress(['t1'], const Narration('Adding the total')),
        isTrue,
      );
      t.workerProgress([
        't1',
      ], const ToolStarted('str_replace', 'Changing index.html'));
      t.workerProgress(['t1'], const OutputTokens(120));
      t.workerProgress([
        't1',
      ], const ToolStarted('render', 'Checking how it looks'));
      t.workerProgress(['t1'], const ToolStarted('finish', 'Publishing'));
      expect(t.step, 'Publishing');
      expect(t.stepCount, 4);
      expect(t.steps, [
        'Changing index.html',
        'Checking how it looks',
        'Publishing',
      ]);
    });

    test('the same step twice in a row counts once', () {
      t.workerWorking(['t1']);
      t.workerProgress(['t1'], const Narration('Publishing'));
      t.workerProgress(['t1'], const ToolStarted('finish', 'Publishing'));
      expect(t.stepCount, 1);
    });

    test("another thread's change is ignored", () {
      t.sent('t1');
      t.workerWorking(['t1']);
      expect(t.workerProgress(['t2'], const Narration('Elsewhere')), isFalse);
      expect(
        t.workerProgress(['t1', 't2'], const Narration('Elsewhere')),
        isFalse,
      );
      expect(t.step, isNull);
      expect(t.stepCount, 0);
    });

    test('a render keeps its screenshot and error count', () {
      t.workerWorking(['t1']);
      final png = Uint8List.fromList([1, 2, 3]);
      t.workerProgress(['t1'], Inspected(png, 2));
      expect(t.inspection?.png, png);
      expect(t.inspection?.errors, 2);
      expect(t.stepCount, 0, reason: 'a render is not a step of its own');
    });

    test('a resent attempt says it is trying again; the first does not', () {
      t.workerWorking(['t1']);
      t.workerProgress(['t1'], const AttemptStarted(1));
      expect(t.step, isNull);
      t.workerProgress(['t1'], const AttemptStarted(2));
      expect(t.step, 'Trying again');
    });

    test('a new run starts its steps afresh', () {
      t.workerWorking(['t1']);
      t.workerProgress(['t1'], const Narration('One'));
      t.workerProgress(['t1'], Inspected(Uint8List(1), 0));
      t.workerLanded(4);
      t.workerWorking(['t2']);
      expect(t.step, isNull);
      expect(t.stepCount, 0);
      expect(t.inspection, isNull);
    });

    test('an interruption says it is waiting for Clax', () {
      t.workerWorking(['t1']);
      t.workerInterrupted();
      expect(t.step, 'Waiting for Clax');
    });

    test('elapsed runs from the first Send or start of the work', () {
      expect(t.since, isNull);
      t.sent('t1');
      final start = now;
      now = now.add(const Duration(seconds: 5));
      t.workerWorking(['t1']);
      expect(t.since, start);
      t.sent('t2');
      expect(t.since, start, reason: 'still the same stretch of work');
      t.workerLanded(4);
      t.frame(4);
      t.workerWorking(['t2']);
      t.workerLanded(5);
      t.frame(5);
      expect(t.working, isFalse);
      expect(t.since, isNull);
      now = now.add(const Duration(seconds: 30));
      t.workerWorking(['t3']);
      expect(t.since, now);
    });

    test('landing: no more steps of that run, until its frame', () {
      t.workerWorking(['t1']);
      t.workerProgress(['t1'], const Narration('One'));
      t.workerLanded(4);
      expect(t.landing, isTrue);
      expect(t.working, isTrue);
      expect(
        t.workerProgress(['t1'], const Narration('Late')),
        isFalse,
        reason: 'the run is over',
      );
      t.frame(4);
      expect(t.landing, isFalse);
      expect(t.working, isFalse);
    });

    test('send→first-step runs from Send to the first step', () {
      t.sent('t1');
      expect(timings.stepLog, ['start']);
      t.workerWorking(['t1']);
      t.workerProgress(['t1'], const AttemptStarted(1));
      t.workerProgress(['t1'], const OutputTokens(10));
      expect(timings.stepLog, ['start']);
      t.workerProgress(['t1'], const ToolStarted('read_file', 'Reading x'));
      t.workerProgress(['t1'], const Narration('Two'));
      expect(timings.stepLog, ['start', 'stop a1']);
    });

    test('send→first-step: a step before the Send records nothing', () {
      t.workerWorking(['t1']);
      t.workerProgress(['t1'], const Narration('One'));
      t.sent('t1');
      t.workerProgress(['t1'], const Narration('Two'));
      expect(timings.stepLog, isEmpty);
    });

    test("send→first-step is not stopped by another change's step", () {
      t.sent('t1');
      t.sent('t2');
      t.workerWorking(['t2']);
      t.workerProgress(['t2'], const Narration('Two'));
      expect(timings.stepLog, ['start'], reason: 't1 was the timed Send');
      t.workerLanded(4);
      t.frame(4);
      t.workerWorking(['t1']);
      t.workerProgress(['t1'], const Narration('One'));
      expect(timings.stepLog, ['start', 'stop a1']);
    });
  });

  group('retry', () {
    test('a failure keeps its threads for Retry', () {
      t.sent('t1');
      t.workerWorking(['t1', 't2']);
      t.workerFailed('The connection dropped');
      expect(t.outcome, ChangeOutcome.failed);
      expect(t.failedThreads, ['t1', 't2']);
      expect(t.retrying, isFalse);
    });

    test('a queued Retry shows the change running again', () {
      t.workerWorking(['t1']);
      t.workerFailed('The connection dropped');
      t.retryStarted();
      expect(t.retrying, isTrue);
      t.retried();
      expect(t.outcome, isNull);
      expect(t.working, isTrue);
      expect(t.again, isTrue);
      expect(t.failedThreads, isEmpty);
      t.workerWorking(['t1']);
      expect(t.again, isFalse);
      expect(t.working, isTrue);
      t.workerLanded(4);
      t.frame(4);
      expect(t.working, isFalse);
      expect(timings.log, isEmpty, reason: 'a Retry is not a Send');
    });

    test('a refused Retry says so and offers no second Retry', () {
      t.workerWorking(['t1']);
      t.workerFailed('The connection dropped');
      t.retryStarted();
      t.retryRefused();
      expect(t.retrying, isFalse);
      expect(t.outcome, ChangeOutcome.failed);
      expect(t.outcomeText, contains('Nothing to run again'));
      expect(t.failedThreads, isEmpty);
    });

    test('dismissing or new work drops the Retry', () {
      t.workerWorking(['t1']);
      t.workerFailed('x');
      t.dismiss();
      expect(t.failedThreads, isEmpty);
      t.workerWorking(['t2']);
      t.workerFailed('y');
      t.sent('t3');
      expect(t.failedThreads, isEmpty);
    });

    test('the worker starting the change before Retry answers wins', () {
      t.workerWorking(['t1']);
      t.workerFailed('x');
      t.retryStarted();
      t.workerWorking(['t1']);
      t.retried();
      expect(t.again, isFalse, reason: 'it is running, not queued');
      t.retryRefused();
      expect(t.outcome, isNull);
      expect(t.working, isTrue);
    });

    test('a failure with no running change offers no Retry', () {
      t.sent('t1');
      t.workerFailed('x');
      expect(t.failedThreads, isEmpty);
    });
  });

  test('threads names the sent threads, then the running change\'s', () {
    expect(t.threads, isEmpty);
    t.sent('t1');
    expect(t.threads, ['t1']);
    t.workerWorking(['t1', 't2']);
    expect(t.threads, ['t1', 't2']);
  });

  test('ended keeps the threads of a change that failed or stopped until '
      'dismissed or new work starts', () {
    t
      ..sent('t1')
      ..workerWorking(['t1'])
      ..workerFailed('x');
    expect(t.threads, isEmpty);
    expect(t.ended, ['t1']);
    t
      ..retryStarted()
      ..retryRefused();
    expect(t.ended, ['t1'], reason: 'a refused Retry keeps what was asked');
    t.dismiss();
    expect(t.ended, isEmpty);
    t
      ..sent('t2')
      ..workerWorking(['t2'])
      ..stopped();
    expect(t.ended, ['t2']);
    t.sent('t3');
    expect(t.ended, isEmpty);
  });

  test('landed keeps the version until new work starts', () {
    t
      ..sent('t1')
      ..workerWorking(['t1']);
    expect(t.landed, isNull);
    t.workerLanded(4);
    t.frame(4);
    expect(t.working, isFalse);
    expect(t.landed, 4);
    t.sent('t2');
    expect(t.landed, isNull);
  });

  test(
    'Stop ends the work in flight; the worker\'s cancel changes nothing',
    () {
      t
        ..sent('t1')
        ..workerWorking(['t1'])
        ..stopped();
      expect(t.working, isFalse);
      expect(t.since, isNull);
      expect(t.outcome, ChangeOutcome.stopped);
      expect(
        t.outcomeText,
        'Stopped. It waits until you run it again or add to the comment.',
      );
      t.workerCancelled();
      expect(
        t.outcomeText,
        'Stopped. It waits until you run it again or add to the comment.',
      );
      expect(t.threads, isEmpty);
    },
  );

  test('a change waiting for the connection says so until it is back', () {
    t.workerWorking(['t1']);
    expect(t.waitingForConnection, isFalse);
    expect(t.workerProgress(['t1'], const WaitingForConnection()), isTrue);
    expect(t.waitingForConnection, isTrue);
    expect(t.step, 'Waiting for connection');
    expect(t.workerProgress(['t1'], const ConnectionBack()), isTrue);
    expect(t.waitingForConnection, isFalse);
    expect(t.step, 'Connection is back');

    t.workerProgress(['t1'], const WaitingForConnection());
    t.workerCancelled();
    expect(t.waitingForConnection, isFalse, reason: 'it ended');
  });
}
