import 'package:clax_app/agent/pricing.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const million = 1000000;

  test('each offered model has its input and output price', () {
    expect(costOf('claude-haiku-5-5', const Usage(input: million)), 0.10);
    expect(costOf('claude-haiku-5-5', const Usage(output: million)), 0.50);
    expect(costOf('claude-sonnet-5-5', const Usage(input: million)), 2);
    expect(costOf('claude-sonnet-5-5', const Usage(output: million)), 10);
    expect(costOf('claude-opus-5-5', const Usage(input: million)), 4);
    expect(costOf('claude-opus-5-5', const Usage(output: million)), 20);
  });

  test('cache reads cost 0.1x input and cache writes 1.25x', () {
    expect(
      costOf('claude-sonnet-5-5', const Usage(cacheRead: million)),
      closeTo(0.20, 1e-12),
    );
    expect(
      costOf('claude-sonnet-5-5', const Usage(cacheWrite: million)),
      closeTo(2.50, 1e-12),
    );
  });

  test('sums every kind of token', () {
    // Haiku 5.5: 0.10 + 0.125 + 0.01 + 0.50 per million each.
    expect(
      costOf(
        'claude-haiku-5-5',
        const Usage(
          input: million,
          cacheWrite: million,
          cacheRead: million,
          output: million,
        ),
      ),
      closeTo(0.735, 1e-12),
    );
  });

  test('an unknown model is priced as Opus', () {
    expect(costOf('claude-next', const Usage(output: million)), 20);
  });
}
