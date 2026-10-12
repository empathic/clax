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

  test('renders are counted against their cap', () {
    final b = RunBudget(RunCaps.change, 'claude-haiku-5-5', StepClock());
    expect(b.canRender, isTrue);
    b.addRender();
    b.addRender();
    expect(b.renders, 2);
    expect(b.canRender, isFalse);
    expect(b.hardStop, isNull, reason: 'renders over cap are refused per call');
  });
}
