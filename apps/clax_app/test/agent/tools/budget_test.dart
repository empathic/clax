import 'package:clax_app/agent/tools/budget.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support.dart';

const note = 'Time is nearly up: call finish now with what works.';

void main() {
  test('the caps of a new app and a change', () {
    expect(RunCaps.newApp.turns, 8);
    expect(RunCaps.newApp.renders, 3);
    expect(RunCaps.newApp.outputTokens, 32000);
    expect(RunCaps.newApp.wallClock, const Duration(seconds: 90));
    expect(RunCaps.newApp.costUsd, 0.50);
    expect(RunCaps.change.turns, 6);
    expect(RunCaps.change.renders, 2);
    expect(RunCaps.change.outputTokens, 20000);
    expect(RunCaps.change.wallClock, const Duration(seconds: 45));
    expect(RunCaps.change.costUsd, 0.50);
  });

  test('usage adds field by field and reads the API usage', () {
    const a = Usage(input: 1, cacheWrite: 2, cacheRead: 3, output: 4);
    expect(
      a + a,
      const Usage(input: 2, cacheWrite: 4, cacheRead: 6, output: 8),
    );
    expect(
      Usage.fromApi({
        'input_tokens': 10,
        'cache_creation_input_tokens': 20,
        'cache_read_input_tokens': 30,
        'output_tokens': 40,
        'server_tool_use': {'web_search_requests': 0},
      }),
      const Usage(input: 10, cacheWrite: 20, cacheRead: 30, output: 40),
    );
    expect(Usage.fromApi(null), const Usage());
  });

  group('hardStop', () {
    test('turns', () {
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', StepClock());
      for (var i = 0; i < 5; i++) {
        b.addTurn(const Usage(output: 1));
      }
      expect(b.hardStop, isNull);
      b.addTurn(const Usage(output: 1));
      expect(b.turns, 6);
      expect(b.hardStop, '6 turns');
    });

    test('wall clock', () {
      final clock = StepClock();
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
      clock.advance(const Duration(seconds: 44));
      expect(b.hardStop, isNull);
      clock.advance(const Duration(seconds: 1));
      expect(b.hardStop, '45 seconds');
      expect(b.timeUp, isTrue);
    });

    test('output tokens', () {
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', StepClock());
      b.addTurn(const Usage(output: 19999));
      expect(b.hardStop, isNull);
      b.addTurn(const Usage(output: 1));
      expect(b.hardStop, '20000 output tokens');
    });

    test('cost', () {
      final b = RunBudget(RunCaps.change, 'claude-opus-5-5', StepClock());
      // 100,000 input tokens on Opus 5.5 cost $0.40.
      b.addTurn(const Usage(input: 100000));
      expect(b.costUsd, closeTo(0.40, 1e-9));
      expect(b.hardStop, isNull);
      b.addTurn(const Usage(input: 25000));
      expect(b.hardStop, r'$0.50');
    });

    test('the clock is paused across an ask wait', () {
      final clock = StepClock();
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
      clock.advance(const Duration(seconds: 10));
      b.pauseClock();
      clock.advance(const Duration(minutes: 10));
      expect(b.elapsed, const Duration(seconds: 10));
      expect(b.hardStop, isNull);
      b.resumeClock();
      clock.advance(const Duration(seconds: 5));
      expect(b.elapsed, const Duration(seconds: 15));
    });

    test('nested pauses resume on the last resume', () {
      final clock = StepClock();
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
      b.pauseClock();
      b.pauseClock();
      clock.advance(const Duration(seconds: 30));
      b.resumeClock();
      clock.advance(const Duration(seconds: 30));
      b.resumeClock();
      clock.advance(const Duration(seconds: 1));
      expect(b.elapsed, const Duration(seconds: 1));
    });
  });

  group('softNote', () {
    test('at two turns left', () {
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', StepClock());
      for (var i = 0; i < 3; i++) {
        b.addTurn(const Usage(output: 1));
      }
      expect(b.softNote, isNull);
      b.addTurn(const Usage(output: 1));
      expect(b.softNote, note);
    });

    test('at 60% of the time', () {
      final clock = StepClock();
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
      clock.advance(const Duration(seconds: 26));
      expect(b.softNote, isNull);
      clock.advance(const Duration(seconds: 1));
      expect(b.softNote, note);
    });

    test('at 60% of the output tokens or the cost', () {
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', StepClock());
      b.addTurn(const Usage(output: 11999));
      expect(b.softNote, isNull);
      b.addTurn(const Usage(output: 1));
      expect(b.softNote, note);

      final c = RunBudget(RunCaps.newApp, 'claude-opus-5-5', StepClock());
      c.addTurn(const Usage(input: 74999));
      expect(c.softNote, isNull);
      c.addTurn(const Usage(input: 1));
      expect(c.softNote, note);
    });
  });

  group('deadline', () {
    test('remaining is the wall clock left, paused time excluded', () {
      final clock = StepClock();
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
      expect(b.remaining, const Duration(seconds: 45));
      clock.advance(const Duration(seconds: 20));
      b.pauseClock();
      clock.advance(const Duration(minutes: 3));
      expect(b.remaining, const Duration(seconds: 25));
      b.resumeClock();
      clock.advance(const Duration(seconds: 60));
      expect(b.remaining, Duration.zero);
    });

    test('whenTimeUp completes when the wall clock runs out, not while an '
        'ask waits', () async {
      final clock = StepClock();
      final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
      var fired = false;
      b.whenTimeUp.then((_) => fired = true);
      clock.advance(const Duration(seconds: 30));
      await pumpEventQueue();
      expect(fired, isFalse);
      b.pauseClock();
      clock.advance(const Duration(minutes: 10));
      await pumpEventQueue();
      expect(fired, isFalse);
      b.resumeClock();
      clock.advance(const Duration(seconds: 14));
      await pumpEventQueue();
      expect(fired, isFalse);
      clock.advance(const Duration(seconds: 1));
      await pumpEventQueue();
      expect(fired, isTrue);
      expect(b.timeUp, isTrue);
    });

    test(
      'whenTimeUp is complete at once when the time is already up',
      () async {
        final clock = StepClock();
        final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
        clock.advance(const Duration(seconds: 50));
        await b.whenTimeUp;
        expect(identical(b.whenTimeUp, b.whenTimeUp), isTrue);
      },
    );
  });

  test('each turn is priced on its own, so the long-prompt rate applies per '
      'request', () {
    final short = RunBudget(RunCaps.newApp, 'claude-haiku-5-5', StepClock());
    short.addTurn(const Usage(input: 60000));
    short.addTurn(const Usage(input: 60000));
    // Two 60K-token prompts at $0.10/MTok.
    expect(short.costUsd, closeTo(0.012, 1e-12));
    final long = RunBudget(RunCaps.newApp, 'claude-haiku-5-5', StepClock());
    long.addTurn(const Usage(input: 120000));
    // One 120K-token prompt at the long-prompt $0.50/MTok.
    expect(long.costUsd, closeTo(0.06, 1e-12));
  });

  test('renders are counted against their cap', () {
    final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', StepClock());
    expect(b.canRender, isTrue);
    b.addRender();
    b.addRender();
    expect(b.renders, 2);
    expect(b.canRender, isFalse);
    expect(b.hardStop, isNull, reason: 'renders over cap are refused per call');
  });

  test('a pause since an earlier time takes that time back, never more '
      'than the clock ran', () async {
    final clock = StepClock();
    final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', clock);
    final start = clock.now();
    clock.advance(const Duration(seconds: 10));
    b.pauseClock();
    clock.advance(const Duration(minutes: 5));
    b.resumeClock();
    clock.advance(const Duration(seconds: 20));
    // A stall since 8 s after the resume: 12 s are taken back.
    b.pauseClock(since: start.add(const Duration(minutes: 5, seconds: 18)));
    expect(b.elapsed, const Duration(seconds: 18));
    b.resumeClock();

    // Since a time before the clock last started: only the running time
    // since then is counted, nothing taken from before.
    clock.advance(const Duration(seconds: 4));
    b.pauseClock(since: start);
    expect(b.elapsed, const Duration(seconds: 18));
    b.resumeClock();

    // Since a time after now: as a pause now.
    clock.advance(const Duration(seconds: 2));
    b.pauseClock(since: clock.now().add(const Duration(hours: 1)));
    expect(b.elapsed, const Duration(seconds: 20));
  });
}
