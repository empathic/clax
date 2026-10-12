/// What a run's tokens cost, from the Messages API's list prices.
library;

import 'tools/budget.dart';

/// Input and output prices of one model, in US dollars per million tokens.
typedef ModelPrice = ({double input, double output});

/// The offered models' prices. Cache reads cost [cacheReadFactor] times the
/// input price, cache writes [cacheWriteFactor] times.
const modelPrices = <String, ModelPrice>{
  'claude-haiku-5-5': (input: 0.10, output: 0.50),
  'claude-sonnet-5-5': (input: 2, output: 10),
  'claude-opus-5-5': (input: 4, output: 20),
};

const cacheReadFactor = 0.1;
const cacheWriteFactor = 1.25;

/// The cost in US dollars of [u] on [model]. A model missing from
/// [modelPrices] is priced as `claude-opus-5-5`, the dearest, so a cost cap
/// never binds late.
double costOf(String model, Usage u) {
  final p = modelPrices[model] ?? modelPrices['claude-opus-5-5']!;
  double per(int tokens, double price) => tokens / 1000000 * price;
  return per(u.input, p.input) +
      per(u.cacheWrite, p.input * cacheWriteFactor) +
      per(u.cacheRead, p.input * cacheReadFactor) +
      per(u.output, p.output);
}
