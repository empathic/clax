import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/ui/change_tracker.dart';
import 'package:flutter_test/flutter_test.dart';

class _Timings implements Timings {
  final log = <String>[];
  @override
  void start(String name) => log.add('start');
  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) {
    log.add('stop v${extra['version']}');
    return null;
  }

  @override
  List<TimingRecord> recent(int n) => const [];
}

void main() {
  late _Timings timings;
  late ChangeTracker t;

  setUp(() {
    timings = _Timings();
    t = ChangeTracker(timings: timings, artifact: 'a1')
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
}
