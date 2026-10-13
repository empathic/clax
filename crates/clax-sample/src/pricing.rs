//! What one round of `sample()` costs, from the Messages API's list prices,
//! for the per-app daily cost cap. Amounts are millionths of a US dollar
//! (micro-USD): a rate in dollars per million tokens times a token count is
//! micro-USD. A model missing from [`rates`] is priced as the dearest one
//! listed, so the cap never binds late.

use super::provider::Usage;

/// One model's rate card, in US dollars per million tokens: `input` and
/// `output`, cache reads at `cache_read` times input and cache writes at
/// `cache_write` times input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// One model's prices: `standard`, and `long` for a request whose prompt
/// (input, cache reads and cache writes together) is over `long_over` tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub standard: Rates,
    pub long: Option<(Rates, u64)>,
}

const fn r(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Rates {
    Rates {
        input,
        output,
        cache_read,
        cache_write,
    }
}

/// The dearest card listed, for any model not listed.
const DEAREST: Price = Price {
    standard: r(5.0, 25.0, 0.1, 1.25),
    long: None,
};

/// The price of `model` (the same cards as the app's agent,
/// `apps/clax_app/lib/agent/pricing.dart`).
pub fn rates(model: &str) -> Price {
    match model {
        "claude-haiku-5-5" => Price {
            standard: r(0.10, 0.50, 0.1, 1.25),
            long: Some((r(0.50, 2.50, 0.1, 1.25), 100_000)),
        },
        "claude-haiku-4-5" => Price {
            standard: r(1.0, 5.0, 0.1, 1.25),
            long: None,
        },
        "claude-sonnet-5-5" => Price {
            standard: r(2.0, 10.0, 0.1, 1.25),
            long: None,
        },
        "claude-opus-5-5" => Price {
            standard: r(4.0, 20.0, 0.05, 1.25),
            long: None,
        },
        _ => DEAREST,
    }
}

/// The cost of one round's `usage` on `model`, in micro-USD, rounded up.
pub fn cost_micro_usd(model: &str, usage: Usage) -> u64 {
    let p = rates(model);
    let prompt = usage.input + usage.cache_read + usage.cache_write;
    let c = match p.long {
        Some((long, over)) if prompt > over => long,
        _ => p.standard,
    };
    let micro = usage.input as f64 * c.input
        + usage.output as f64 * c.output
        + usage.cache_read as f64 * c.input * c.cache_read
        + usage.cache_write as f64 * c.input * c.cache_write;
    micro.ceil() as u64
}

/// `usd` as micro-USD (a configured cap).
pub fn micro(usd: f64) -> u64 {
    (usd * 1_000_000.0).round().max(0.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haiku_is_priced_per_card_and_long_prompts_at_the_long_rate() {
        let u = Usage {
            input: 1_000,
            output: 2_000,
            cache_read: 0,
            cache_write: 0,
        };
        // 1000 × 0.10 + 2000 × 0.50 = 1100 micro-USD.
        assert_eq!(cost_micro_usd("claude-haiku-5-5", u), 1_100);
        let long = Usage {
            input: 200_000,
            ..Usage::default()
        };
        assert_eq!(cost_micro_usd("claude-haiku-5-5", long), 100_000);
        let cached = Usage {
            cache_read: 10_000,
            ..Usage::default()
        };
        assert_eq!(cost_micro_usd("claude-sonnet-5-5", cached), 2_000);
    }

    #[test]
    fn an_unknown_model_is_priced_as_the_dearest() {
        let u = Usage {
            input: 1,
            output: 1,
            ..Usage::default()
        };
        assert_eq!(cost_micro_usd("claude-next", u), 30);
        assert_eq!(micro(0.5), 500_000);
    }
}
