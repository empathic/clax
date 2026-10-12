import 'package:clax_app/agent/pricing.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const million = 1000000;

  test('each offered model has its input and output price', () {
    expect(
      costOf('claude-haiku-5-5', const Usage(input: 100000)),
      closeTo(0.01, 1e-12),
    );
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

  test('Opus 5.5 cache reads cost 0.05x input', () {
    expect(
      costOf('claude-opus-5-5', const Usage(cacheRead: million)),
      closeTo(0.20, 1e-12),
    );
    expect(
      costOf('claude-opus-5-5', const Usage(cacheWrite: million)),
      closeTo(5.00, 1e-12),
    );
  });

  test('Haiku 5.5 bills a prompt over 100K tokens at \$0.50/\$2.50', () {
    // 100K prompt tokens: the standard rate.
    expect(
      costOf('claude-haiku-5-5', const Usage(input: 100000, output: 1000)),
      closeTo(0.01 + 0.0005, 1e-12),
    );
    // Over 100K counting cache reads and writes: the long-prompt rate for
    // every token of the request.
    expect(
      costOf(
        'claude-haiku-5-5',
        const Usage(
          input: 1000,
          cacheRead: 90000,
          cacheWrite: 10000,
          output: 1000,
        ),
      ),
      closeTo(
        1000 * 0.50 / 1e6 +
            90000 * 0.05 / 1e6 +
            10000 * 0.625 / 1e6 +
            1000 * 2.50 / 1e6,
        1e-12,
      ),
    );
  });

  test('sums every kind of token of one request', () {
    // Sonnet 5.5: 2 + 2.50 + 0.20 + 10 per million each.
    expect(
      costOf(
        'claude-sonnet-5-5',
        const Usage(
          input: million,
          cacheWrite: million,
          cacheRead: million,
          output: million,
        ),
      ),
      closeTo(14.70, 1e-12),
    );
  });

  test('an unknown model is priced as Opus', () {
    expect(costOf('claude-next', const Usage(output: million)), 20);
  });
}
