/// What a run's tokens cost, from the Messages API's list prices.
library;

import 'tools/budget.dart';

/// One model's rate card, in US dollars per million tokens: [input] and
/// [output], cache reads at [cacheRead] times input, cache writes at
/// [cacheWrite] times input.
typedef Rates = ({
  double input,
  double output,
  double cacheRead,
  double cacheWrite,
});

/// One model's prices: [standard], and [long] for a request whose prompt
/// (input, cache reads and cache writes together) is over [longOver]
/// tokens; null [long]: one rate card for every request.
typedef ModelPrice = ({Rates standard, Rates? long, int longOver});

/// The offered models' prices.
const modelPrices = <String, ModelPrice>{
  'claude-haiku-5-5': (
    standard: (input: 0.10, output: 0.50, cacheRead: 0.1, cacheWrite: 1.25),
    long: (input: 0.50, output: 2.50, cacheRead: 0.1, cacheWrite: 1.25),
    longOver: 100000,
  ),
  'claude-sonnet-5-5': (
    standard: (input: 2, output: 10, cacheRead: 0.1, cacheWrite: 1.25),
    long: null,
    longOver: 0,
  ),
  'claude-opus-5-5': (
    standard: (input: 4, output: 20, cacheRead: 0.05, cacheWrite: 1.25),
    long: null,
    longOver: 0,
  ),
};

/// The cost in US dollars of one request's usage [u] on [model]; a run's
/// cost is the sum over its requests, since the long-prompt rate is
/// chosen per request. A model missing from [modelPrices] is priced as
/// `claude-opus-5-5`, the dearest, so a cost cap never binds late.
double costOf(String model, Usage u) {
  final p = modelPrices[model] ?? modelPrices['claude-opus-5-5']!;
  final prompt = u.input + u.cacheRead + u.cacheWrite;
  final r = switch (p.long) {
    final long? when prompt > p.longOver => long,
    _ => p.standard,
  };
  double per(int tokens, double price) => tokens / 1000000 * price;
  return per(u.input, r.input) +
      per(u.cacheWrite, r.input * r.cacheWrite) +
      per(u.cacheRead, r.input * r.cacheRead) +
      per(u.output, r.output);
}
