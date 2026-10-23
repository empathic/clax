//! Credits (migration 50, spec 2026-10-21-claxbot-design §7): what each
//! underlying cost is priced at (cost lines), what was spent (the usage
//! ledger), each person's weekly allowance and lift, and the reports the
//! admin pages show.
//!
//! # Units
//!
//! No floats: credits are integer **milli-credits**, money integer
//! **micro-USD**, margins integer **basis points** (100 bp = 1 %). A cost
//! line prices one *line unit* ([`Unit`]: a million tokens, a GiB-second
//! of script compute, …); a usage row's `qty` is in the unit's *base*
//! ([`Unit::base`]: tokens, MiB·ms, requests, bytes·s, runs).
//!
//! - **A row's credits** = `qty` × cost × (1 + margin) ÷ (credit price ×
//!   the unit's base), computed exactly in integers and rounded up to the
//!   milli-credit once, for the row ([`row_credits_milli`]); a line with
//!   `fixed_credits_milli` charges `qty` × that ÷ the base instead (the
//!   per-run harness fee). Its **cost** is `qty` × cost ÷ the base, rounded
//!   up. The per-unit rate shown to admins ([`credits_per_unit_milli`]) is
//!   rounded up for display only and never charged from.
//! - The exact inputs are snapshotted on the row (cost per unit, margin,
//!   fixed credits, the credit price, the unit's base), so a later price
//!   change never reprices history.
//! - A line that `applies` only to the instance key charges nothing on a
//!   person's own key: the row is written, with cost and credits 0.
//!
//! # Weeks
//!
//! A week runs from Monday 00:00 UTC to the next ([`week_start`]). Every
//! time is the caller's or the store's clock, never the system's. A
//! person's weekly credits are the instance free tier
//! ([`Store::free_weekly_milli`]) plus their lift; a row's `free_milli` is
//! the part of its credits that fell within the allowance when it was
//! charged, the rest being paid (revenue).

use super::Store;
use super::bot_settings::{Overrides, Setting, int_setting_in};
use super::roles::{check_live_person, set_by};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use std::collections::BTreeMap;

/// The unit a cost line prices.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// A million input tokens.
    MtokInput,
    /// A million cached input tokens (cache reads).
    MtokCachedInput,
    /// A million input tokens written to the cache (5-minute writes).
    MtokCacheWrite,
    /// A million output tokens.
    MtokOutput,
    /// A GiB-second of script compute: CPU time × memory.
    CpuGbS,
    /// A million requests (Durable Object requests).
    Mreq,
    /// A GiB stored for 30 days (Toolpath storage).
    GbMonth,
    /// One run (the harness fee).
    Run,
}

impl Unit {
    /// Every unit.
    pub const ALL: &'static [Unit] = &[
        Unit::MtokInput,
        Unit::MtokCachedInput,
        Unit::MtokCacheWrite,
        Unit::MtokOutput,
        Unit::CpuGbS,
        Unit::Mreq,
        Unit::GbMonth,
        Unit::Run,
    ];

    /// The unit's stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::MtokInput => "mtok_input",
            Unit::MtokCachedInput => "mtok_cached_input",
            Unit::MtokCacheWrite => "mtok_cache_write",
            Unit::MtokOutput => "mtok_output",
            Unit::CpuGbS => "cpu_gb_s",
            Unit::Mreq => "mreq",
            Unit::GbMonth => "gb_month",
            Unit::Run => "run",
        }
    }

    /// The unit named `s`.
    pub fn parse(s: &str) -> Option<Unit> {
        Unit::ALL.iter().copied().find(|u| u.as_str() == s)
    }

    /// How many base quantities one line unit is: 10^6 tokens, 1024 MiB ×
    /// 1000 ms, 10^6 requests, 2^30 bytes × 30 days of seconds, one run.
    pub fn base(self) -> i64 {
        match self {
            Unit::MtokInput
            | Unit::MtokCachedInput
            | Unit::MtokCacheWrite
            | Unit::MtokOutput
            | Unit::Mreq => 1_000_000,
            Unit::CpuGbS => 1024 * 1000,
            Unit::GbMonth => (1 << 30) * 30 * 86_400,
            Unit::Run => 1,
        }
    }

    /// Whether the unit counts tokens.
    pub fn is_tokens(self) -> bool {
        matches!(
            self,
            Unit::MtokInput | Unit::MtokCachedInput | Unit::MtokCacheWrite | Unit::MtokOutput
        )
    }
}

/// Which keys a cost line charges on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    /// Only usage on the instance key (model tokens): on a person's own
    /// key the row is written with cost and credits 0.
    InstanceKey,
    /// Every usage, whoever pays the provider (compute, storage, the fee).
    Always,
}

impl Applies {
    pub fn as_str(self) -> &'static str {
        match self {
            Applies::InstanceKey => "instance_key",
            Applies::Always => "always",
        }
    }

    pub fn parse(s: &str) -> Option<Applies> {
        match s {
            "instance_key" => Some(Applies::InstanceKey),
            "always" => Some(Applies::Always),
            _ => None,
        }
    }
}

/// Whose model key a usage was on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    Instance,
    Own,
}

impl KeySource {
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Instance => "instance",
            KeySource::Own => "own",
        }
    }

    pub fn parse(s: &str) -> Option<KeySource> {
        match s {
            "instance" => Some(KeySource::Instance),
            "own" => Some(KeySource::Own),
            _ => None,
        }
    }
}

/// One cost line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CostLine {
    /// A stable slug (`claude-sonnet-5-5.input`, `script.compute`).
    pub id: String,
    pub label: String,
    pub unit: Unit,
    /// What one line unit costs Clax.
    pub cost_micro_usd: i64,
    pub margin_bp: i64,
    /// Credits per line unit charged whatever the cost (the harness fee);
    /// `None`: priced from cost and margin.
    pub fixed_credits_milli: Option<i64>,
    pub applies: Applies,
    /// Where the cost figure comes from.
    pub source: String,
    /// The model a token line prices.
    pub model: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
}

impl CostLine {
    /// Credits per line unit at `price_micro_usd` a credit, rounded up: for
    /// display. Charges are computed exactly per row ([`row_credits_milli`]).
    pub fn credits_per_unit_milli(&self, price_micro_usd: i64) -> i64 {
        self.fixed_credits_milli.unwrap_or_else(|| {
            credits_per_unit_milli(self.cost_micro_usd, self.margin_bp, price_micro_usd)
        })
    }
}

/// `ceil(a / b)` for `a >= 0`, `b > 0`, for display rates (saturating at
/// the caller).
fn div_ceil(a: i128, b: i128) -> i128 {
    a.saturating_add(b - 1) / b
}

/// Credits, in milli-credits, one line unit costing `cost_micro_usd` at
/// `margin_bp` charges at `price_micro_usd` a credit: cost × (1 + margin) ÷
/// price, rounded up to the milli-credit. A price below 1 is taken as 1.
pub fn credits_per_unit_milli(cost_micro_usd: i64, margin_bp: i64, price_micro_usd: i64) -> i64 {
    let num = i128::from(cost_micro_usd.max(0))
        .saturating_mul(i128::from(10_000i64.saturating_add(margin_bp.max(0))))
        .saturating_mul(1000);
    let den = 10_000i128.saturating_mul(i128::from(price_micro_usd.max(1)));
    i64::try_from(div_ceil(num, den)).unwrap_or(i64::MAX)
}

/// The most a cost line may cost per line unit: $10 000 (a million tokens,
/// a GiB-second, a run).
pub const MAX_COST_MICRO_USD: i64 = 10_000_000_000;
/// The highest margin: 1 000 000 bp, a hundred times the cost.
pub const MAX_MARGIN_BP: i64 = 1_000_000;
/// The most credits a fixed line may charge per line unit: a million.
pub const MAX_FIXED_CREDITS_MILLI: i64 = 1_000_000_000;

/// `charge_overflow`: a charge whose arithmetic leaves the integers it is
/// kept in.
fn overflow() -> CoreError {
    CoreError::invalid(
        "charge_overflow",
        "a usage's charge is too large to compute; its quantity or its line's figures are out of range",
    )
}

/// `ceil(a / b)` for `a >= 0`, `b > 0`, checked.
fn div_ceil_checked(a: i128, b: i128) -> Result<i128> {
    a.checked_add(b - 1).map(|n| n / b).ok_or_else(overflow)
}

/// The milli-credits `qty` base quantities of a line unit of `base` charge:
/// `fixed` per line unit when set, else priced from `cost_micro_usd` per
/// line unit at `margin_bp` and `price_micro_usd` a credit, computed exactly
/// and rounded up once: ⌈qty × cost × (10000 + margin) × 1000 ÷ (10000 ×
/// price × base)⌉. A price below 1 is taken as 1. Every step is checked:
/// within the cost line bounds ([`MAX_COST_MICRO_USD`], [`MAX_MARGIN_BP`],
/// [`MAX_FIXED_CREDITS_MILLI`]) the product of any `i64` quantity fits in
/// `i128`, and a result beyond `i64` is refused, never wrapped.
///
/// # Errors
/// `charge_overflow`.
pub fn row_credits_milli(
    qty: i64,
    cost_micro_usd: i64,
    margin_bp: i64,
    fixed: Option<i64>,
    price_micro_usd: i64,
    base: i64,
) -> Result<i64> {
    let q = i128::from(qty.max(0));
    let base = i128::from(base.max(1));
    let v = match fixed {
        Some(f) => div_ceil_checked(
            q.checked_mul(i128::from(f.max(0))).ok_or_else(overflow)?,
            base,
        )?,
        None => {
            let num = q
                .checked_mul(i128::from(cost_micro_usd.max(0)))
                .and_then(|n| n.checked_mul(i128::from(10_000 + margin_bp.max(0))))
                .and_then(|n| n.checked_mul(1000))
                .ok_or_else(overflow)?;
            let den = i128::from(price_micro_usd.max(1))
                .checked_mul(10_000)
                .and_then(|d| d.checked_mul(base))
                .ok_or_else(overflow)?;
            div_ceil_checked(num, den)?
        }
    };
    i64::try_from(v).map_err(|_| overflow())
}

/// `qty` base quantities of `per_unit` (a line-unit rate) in a unit of
/// `base`, rounded up, checked.
///
/// # Errors
/// `charge_overflow`.
fn scaled(qty: i64, per_unit: i64, base: i64) -> Result<i64> {
    let n = i128::from(qty.max(0))
        .checked_mul(i128::from(per_unit.max(0)))
        .ok_or_else(overflow)?;
    i64::try_from(div_ceil_checked(n, i128::from(base.max(1)))?).map_err(|_| overflow())
}

/// The start of the week `t` falls in: the Monday 00:00 UTC on or before it.
pub fn week_start(t: DateTime<Utc>) -> DateTime<Utc> {
    let d = t.date_naive() - Duration::days(i64::from(t.weekday().num_days_from_monday()));
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight exists"))
}

/// The end of the week `start` begins: the next Monday 00:00 UTC.
pub fn week_end(start: DateTime<Utc>) -> DateTime<Utc> {
    week_start(start) + Duration::days(7)
}

/// The key of the week `t` falls in, as usage rows store it: its start in
/// RFC 3339 (`2026-10-19T00:00:00Z`).
pub fn week_key(t: DateTime<Utc>) -> String {
    week_start(t).format("%Y-%m-%dT00:00:00Z").to_string()
}

/// One metered quantity to charge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metered {
    /// The cost line's ID.
    pub line: String,
    /// In the line unit's base ([`Unit::base`]).
    pub qty: i64,
    pub key_source: KeySource,
    /// The instance bears the cost and the person is charged nothing: the
    /// row keeps its cost with 0 credits (a model call cut off by a crash,
    /// a lost claim or a stream cut, which the step then made again).
    pub absorbed: bool,
    /// The quantity is an estimate (a cut stream's output, counted from
    /// what it streamed), not a figure the provider reported.
    pub estimated: bool,
    /// The rates to charge at, snapshotted when the usage happened; `None`
    /// charges at the line's rates now.
    pub rate: Option<Rate>,
}

/// A cost line's rates and the credit price, as a usage is charged at
/// them: snapshotted when a model call's usage is observed
/// ([`Store::rate_of`]), so its row is priced as then even when the line
/// changed or went since.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Rate {
    pub unit: Unit,
    pub cost_micro_usd: i64,
    pub margin_bp: i64,
    pub fixed_credits_milli: Option<i64>,
    pub applies: Applies,
    pub credit_price_micro_usd: i64,
}

impl Metered {
    pub fn new(line: impl Into<String>, qty: i64, key_source: KeySource) -> Metered {
        Metered {
            line: line.into(),
            qty,
            key_source,
            absorbed: false,
            estimated: false,
            rate: None,
        }
    }

    /// [`Metered::new`], its cost absorbed by the instance
    /// ([`Metered::absorbed`]).
    pub fn absorbed(line: impl Into<String>, qty: i64, key_source: KeySource) -> Metered {
        Metered {
            absorbed: true,
            ..Metered::new(line, qty, key_source)
        }
    }
}

/// One usage row, as written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsageRow {
    pub id: i64,
    pub person: String,
    pub run_id: Option<String>,
    pub step_n: Option<i64>,
    pub line: String,
    pub unit: Unit,
    pub qty: i64,
    /// The line's cost per line unit when charged.
    pub rate_cost_micro_usd: i64,
    pub margin_bp: i64,
    /// The line's fixed credits per line unit when charged.
    pub fixed_credits_milli: Option<i64>,
    pub credit_price_micro_usd: i64,
    /// Base quantities to the line unit ([`Unit::base`]) when charged.
    pub base_qty: i64,
    pub cost_micro_usd: i64,
    pub credits_milli: i64,
    /// The part of `credits_milli` drawn from the weekly allowance.
    pub free_milli: i64,
    pub key_source: KeySource,
    /// The instance bore the cost: `credits_milli` is 0 ([`Metered::absorbed`]).
    pub absorbed: bool,
    /// `qty` is an estimate ([`Metered::estimated`]).
    pub estimated: bool,
    /// The week's key ([`week_key`]).
    pub week: String,
    pub at: String,
}

impl UsageRow {
    /// Whether the row was drawn entirely from the free allowance.
    pub fn free(&self) -> bool {
        self.free_milli == self.credits_milli
    }
}

const USAGE_COLS: &str = "id, person, run_id, step_n, line, unit, qty, rate_cost_micro_usd,
    margin_bp, fixed_credits_milli, credit_price_micro_usd, cost_micro_usd, credits_milli,
    free_milli, key_source, week, at, base_qty, absorbed, estimated";

fn row_to_usage(r: &Row<'_>) -> rusqlite::Result<UsageRow> {
    let unit: String = r.get(5)?;
    let ks: String = r.get(14)?;
    Ok(UsageRow {
        id: r.get(0)?,
        person: r.get(1)?,
        run_id: r.get(2)?,
        step_n: r.get(3)?,
        line: r.get(4)?,
        unit: Unit::parse(&unit).unwrap_or(Unit::Run),
        qty: r.get(6)?,
        rate_cost_micro_usd: r.get(7)?,
        margin_bp: r.get(8)?,
        fixed_credits_milli: r.get(9)?,
        credit_price_micro_usd: r.get(10)?,
        base_qty: r.get(17)?,
        cost_micro_usd: r.get(11)?,
        credits_milli: r.get(12)?,
        free_milli: r.get(13)?,
        key_source: KeySource::parse(&ks).unwrap_or(KeySource::Instance),
        week: r.get(15)?,
        at: r.get(16)?,
        absorbed: r.get(18)?,
        estimated: r.get(19)?,
    })
}

const LINE_COLS: &str = "id, label, unit, cost_micro_usd, margin_bp, fixed_credits_milli, applies,
    source, model, updated_by, updated_at";

fn row_to_line(r: &Row<'_>) -> rusqlite::Result<CostLine> {
    let unit: String = r.get(2)?;
    let applies: String = r.get(6)?;
    Ok(CostLine {
        id: r.get(0)?,
        label: r.get(1)?,
        unit: Unit::parse(&unit).unwrap_or(Unit::Run),
        cost_micro_usd: r.get(3)?,
        margin_bp: r.get(4)?,
        fixed_credits_milli: r.get(5)?,
        applies: Applies::parse(&applies).unwrap_or(Applies::Always),
        source: r.get(7)?,
        model: r.get(8)?,
        updated_by: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

fn line_in(c: &Connection, id: &str) -> Result<Option<CostLine>> {
    Ok(
        c.prepare_cached(&format!("SELECT {LINE_COLS} FROM cost_lines WHERE id = ?1"))?
            .query_row(params![id], row_to_line)
            .optional()?,
    )
}

/// The margin seeded on token lines (40 %) and on infrastructure lines
/// (90 %).
pub const TOKEN_MARGIN_BP: i64 = 4_000;
pub const INFRA_MARGIN_BP: i64 = 9_000;

/// The token units every model a run uses must have a line for.
pub const TOKEN_UNITS: [Unit; 4] = [
    Unit::MtokInput,
    Unit::MtokCachedInput,
    Unit::MtokCacheWrite,
    Unit::MtokOutput,
];

/// The cost line ID of `model`'s tokens of `unit` (`claude-sonnet-5-5.input`,
/// `.cached_input`, `.cache_write`, `.output`).
pub fn token_line(model: &str, unit: Unit) -> String {
    let part = match unit {
        Unit::MtokInput => "input",
        Unit::MtokCachedInput => "cached_input",
        Unit::MtokCacheWrite => "cache_write",
        _ => "output",
    };
    format!("{model}.{part}")
}

/// `model_not_priced` unless `model` has a cost line for each of
/// [`TOKEN_UNITS`] on `c`, so a run on it can charge every turn.
pub(crate) fn check_model_priced(c: &Connection, model: &str) -> Result<()> {
    let missing: Vec<String> = TOKEN_UNITS
        .iter()
        .map(|u| token_line(model, *u))
        .filter(|id| !matches!(line_in(c, id), Ok(Some(_))))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "model_not_priced",
            format!(
                "model {model} has no price: an admin sets its cost lines ({}) first",
                missing.join(", ")
            ),
        ))
    }
}

/// A model's token prices, in micro-USD per million tokens, for
/// [`Store::price_model`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelPrices {
    /// Shown in the lines' labels (`Claude Sonnet 5.5`).
    pub name: String,
    pub input: i64,
    pub cached_input: i64,
    pub cache_write: i64,
    pub output: i64,
    pub margin_bp: i64,
    pub source: String,
}

/// The infrastructure lines' IDs.
pub const LINE_SCRIPT_COMPUTE: &str = "script.compute";
pub const LINE_DO_REQUESTS: &str = "do.requests";
pub const LINE_TOOLPATH_STORAGE: &str = "toolpath.storage";
pub const LINE_HARNESS_FEE: &str = "harness.run";

/// The models seeded with token lines, with list prices in micro-USD per
/// million input, cached input (cache read) and output tokens.
const SEED_MODELS: [(&str, &str, i64, i64, i64); 4] = [
    (
        "claude-haiku-5-5",
        "Claude Haiku 5.5",
        100_000,
        10_000,
        500_000,
    ),
    (
        "claude-sonnet-5-5",
        "Claude Sonnet 5.5",
        2_000_000,
        200_000,
        10_000_000,
    ),
    (
        "claude-opus-5-5",
        "Claude Opus 5.5",
        4_000_000,
        200_000,
        20_000_000,
    ),
    (
        "claude-fable-5-1",
        "Claude Fable 5.1",
        10_000_000,
        250_000,
        50_000_000,
    ),
];

/// The cache-write price of a model whose input costs `input`: 1.25 times
/// it, rounded up, the list price of a 5-minute cache write.
pub fn cache_write_price(input: i64) -> i64 {
    (input * 5 + 3) / 4
}

/// Seeds the cost lines (migration 50's Rust step): input, cached input,
/// cache-write and output tokens for each current Claude model at its list
/// price, script compute, Durable Object requests, Toolpath storage and
/// the per-run harness fee, at the default margins. Every figure's `source`
/// says where it comes from and that it is to be checked.
pub(crate) fn seed_cost_lines(tx: &Transaction<'_>) -> Result<()> {
    let mut insert = tx.prepare(
        "INSERT OR IGNORE INTO cost_lines (id, label, unit, cost_micro_usd, margin_bp,
            fixed_credits_milli, applies, source, model, updated_by, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'migration', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
    )?;
    for (model, name, input, cached, output) in SEED_MODELS {
        for (unit, what, cost) in [
            (Unit::MtokInput, "input", input),
            (Unit::MtokCachedInput, "cached input", cached),
            (
                Unit::MtokCacheWrite,
                "cache write",
                cache_write_price(input),
            ),
            (Unit::MtokOutput, "output", output),
        ] {
            let source = match unit {
                Unit::MtokCachedInput if model == "claude-haiku-5-5" => {
                    "Anthropic list price as of 2026-10-06 (cache read taken as a tenth of input; check)"
                }
                Unit::MtokCacheWrite => {
                    "1.25 times the input list price as of 2026-10-06, for 5-minute cache writes (check)"
                }
                _ => "Anthropic list price as of 2026-10-06 (check)",
            };
            insert.execute(params![
                token_line(model, unit),
                format!("{name} {what} tokens"),
                unit.as_str(),
                cost,
                TOKEN_MARGIN_BP,
                Option::<i64>::None,
                Applies::InstanceKey.as_str(),
                source,
                model,
            ])?;
        }
    }
    for (id, label, unit, cost, fixed, source) in [
        (
            LINE_SCRIPT_COMPUTE,
            "Script compute",
            Unit::CpuGbS,
            17,
            None,
            "estimate: $0.0000166667 per GB-s, serverless compute list price (check)",
        ),
        (
            LINE_DO_REQUESTS,
            "Durable Object requests",
            Unit::Mreq,
            150_000,
            None,
            "Cloudflare Durable Objects list price, $0.15 per million requests (check)",
        ),
        (
            LINE_TOOLPATH_STORAGE,
            "Toolpath storage",
            Unit::GbMonth,
            200_000,
            None,
            "Cloudflare Durable Objects SQLite storage list price, $0.20 per GB-month (check)",
        ),
        (
            LINE_HARNESS_FEE,
            "ClaxBot run",
            Unit::Run,
            0,
            Some(1_000i64),
            "a fixed fee per run, no underlying cost",
        ),
    ] {
        insert.execute(params![
            id,
            label,
            unit.as_str(),
            cost,
            INFRA_MARGIN_BP,
            fixed,
            Applies::Always.as_str(),
            source,
            Option::<String>::None,
        ])?;
    }
    Ok(())
}

/// A person's credits for one week.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WeekUsage {
    /// The week's start and end (RFC 3339, Monday 00:00 UTC).
    pub start: String,
    pub end: String,
    /// The free tier plus the lift.
    pub weekly_milli: i64,
    pub lift_milli: i64,
    pub spent_milli: i64,
    /// Spent at least 80 % of the weekly credits.
    pub warn: bool,
    /// Spent all of them.
    pub exhausted: bool,
}

impl WeekUsage {
    /// What is left, never below 0.
    pub fn remaining_milli(&self) -> i64 {
        (self.weekly_milli - self.spent_milli).max(0)
    }
}

/// `person`'s lift on `c`, 0 without one.
fn lift_in(c: &Connection, person: &str) -> Result<i64> {
    Ok(
        c.prepare_cached("SELECT lift_milli FROM credit_lifts WHERE person = ?1")?
            .query_row(params![person], |r| r.get(0))
            .optional()?
            .unwrap_or(0),
    )
}

/// The credits `person` spent in week `week` (a [`week_key`]) on `c`.
fn spent_in(c: &Connection, person: &str, week: &str) -> Result<i64> {
    Ok(c.prepare_cached(
        "SELECT COALESCE(SUM(credits_milli), 0) FROM usage_rows WHERE person = ?1 AND week = ?2",
    )?
    .query_row(params![person, week], |r| r.get(0))?)
}

pub(crate) fn week_usage_in(
    c: &Connection,
    ov: &Overrides,
    person: &str,
    at: DateTime<Utc>,
) -> Result<WeekUsage> {
    let start = week_start(at);
    let lift = lift_in(c, person)?;
    let weekly = int_setting_in(c, ov, Setting::FreeWeeklyMilli)? + lift;
    let spent = spent_in(c, person, &week_key(at))?;
    let fmt = |t: DateTime<Utc>| t.format("%Y-%m-%dT00:00:00Z").to_string();
    Ok(WeekUsage {
        start: fmt(start),
        end: fmt(week_end(start)),
        weekly_milli: weekly,
        lift_milli: lift,
        spent_milli: spent,
        // spent >= 80 % of weekly, in integers.
        warn: spent * 5 >= weekly * 4,
        exhausted: spent >= weekly,
    })
}

/// Charges `items` to `person` at `at`, for step `step` of run `run` when
/// given, on `tx`: one usage row each, in order, its rates read from its
/// cost line and snapshotted, its free part what the person's allowance
/// had left. Returns the rows.
///
/// # Errors
/// `unknown_cost_line` for an item whose line does not exist;
/// `invalid_usage` for a negative quantity; the store's.
pub(crate) fn charge_in(
    tx: &Transaction<'_>,
    ov: &Overrides,
    person: &str,
    run: Option<(&str, i64)>,
    items: &[Metered],
    at: DateTime<Utc>,
) -> Result<Vec<UsageRow>> {
    let price = int_setting_in(tx, ov, Setting::CreditPriceMicroUsd)?;
    let week = week_key(at);
    let stamp = clax_core::time::stamp(at);
    let usage = week_usage_in(tx, ov, person, at)?;
    let mut left = usage.weekly_milli - usage.spent_milli;
    let mut out = Vec::with_capacity(items.len());
    for m in items {
        if m.qty < 0 {
            return Err(CoreError::invalid(
                "invalid_usage",
                format!("a usage of {} has a negative quantity", m.line),
            ));
        }
        let r = match m.rate {
            Some(r) => r,
            None => {
                let line = line_in(tx, &m.line)?.ok_or_else(|| {
                    CoreError::invalid(
                        "unknown_cost_line",
                        format!("no cost line is named '{}'", m.line),
                    )
                })?;
                Rate {
                    unit: line.unit,
                    cost_micro_usd: line.cost_micro_usd,
                    margin_bp: line.margin_bp,
                    fixed_credits_milli: line.fixed_credits_milli,
                    applies: line.applies,
                    credit_price_micro_usd: price,
                }
            }
        };
        let base = r.unit.base();
        let waived = r.applies == Applies::InstanceKey && m.key_source == KeySource::Own;
        let (cost, credits) = if waived {
            (0, 0)
        } else if m.absorbed {
            (scaled(m.qty, r.cost_micro_usd, base)?, 0)
        } else {
            (
                scaled(m.qty, r.cost_micro_usd, base)?,
                row_credits_milli(
                    m.qty,
                    r.cost_micro_usd,
                    r.margin_bp,
                    r.fixed_credits_milli,
                    r.credit_price_micro_usd,
                    base,
                )?,
            )
        };
        let free = credits.min(left.max(0));
        left = left.saturating_sub(credits);
        tx.prepare_cached(
            "INSERT INTO usage_rows (person, run_id, step_n, line, unit, qty, rate_cost_micro_usd,
                margin_bp, fixed_credits_milli, credit_price_micro_usd, base_qty, cost_micro_usd,
                credits_milli, free_milli, key_source, week, at, absorbed, estimated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        )?
        .execute(params![
            person,
            run.map(|r| r.0),
            run.map(|r| r.1),
            m.line,
            r.unit.as_str(),
            m.qty,
            r.cost_micro_usd,
            r.margin_bp,
            r.fixed_credits_milli,
            r.credit_price_micro_usd,
            base,
            cost,
            credits,
            free,
            m.key_source.as_str(),
            week,
            stamp,
            m.absorbed,
            m.estimated,
        ])?;
        out.push(UsageRow {
            id: tx.last_insert_rowid(),
            person: person.to_string(),
            run_id: run.map(|r| r.0.to_string()),
            step_n: run.map(|r| r.1),
            line: m.line.clone(),
            unit: r.unit,
            qty: m.qty,
            rate_cost_micro_usd: r.cost_micro_usd,
            margin_bp: r.margin_bp,
            fixed_credits_milli: r.fixed_credits_milli,
            credit_price_micro_usd: r.credit_price_micro_usd,
            base_qty: base,
            cost_micro_usd: cost,
            credits_milli: credits,
            free_milli: free,
            key_source: m.key_source,
            absorbed: m.absorbed,
            estimated: m.estimated,
            week: week.clone(),
            at: stamp.clone(),
        });
    }
    Ok(out)
}

/// A new cost line, or the fields of one to change (each `Some` is set).
#[derive(Clone, Debug, Default)]
pub struct CostLinePatch {
    pub label: Option<String>,
    pub unit: Option<Unit>,
    pub cost_micro_usd: Option<i64>,
    pub margin_bp: Option<i64>,
    /// `Some(None)` clears a fixed charge.
    pub fixed_credits_milli: Option<Option<i64>>,
    pub applies: Option<Applies>,
    pub source: Option<String>,
    /// `Some(None)` clears the model.
    pub model: Option<Option<String>>,
}

/// `id` checked as a cost line ID: 1 to 160 of `[A-Za-z0-9._:-]` (a
/// model's token lines are its ID and a suffix).
fn check_line_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 160
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "invalid_cost_line",
            "a cost line ID is 1 to 160 letters, digits, '.', '_', ':' or '-'",
        ))
    }
}

/// `text` trimmed, 1 to `max` characters, for the field `what`.
fn check_text(what: &str, text: &str, max: usize) -> Result<String> {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > max || t.chars().any(char::is_control) {
        return Err(CoreError::invalid(
            "invalid_cost_line",
            format!("a cost line's {what} is 1 to {max} characters"),
        ));
    }
    Ok(t.to_string())
}

fn check_amount(what: &str, v: i64, max: i64) -> Result<i64> {
    if (0..=max).contains(&v) {
        Ok(v)
    } else {
        Err(CoreError::invalid(
            "invalid_cost_line",
            format!("a cost line's {what} is a whole number from 0 to {max}"),
        ))
    }
}

/// Totals over usage rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub qty: i64,
    pub cost_micro_usd: i64,
    pub credits_milli: i64,
    pub free_milli: i64,
}

impl Totals {
    fn add(&mut self, r: &UsageRow) {
        self.qty += r.qty;
        self.cost_micro_usd += r.cost_micro_usd;
        self.credits_milli += r.credits_milli;
        self.free_milli += r.free_milli;
    }
}

/// One run's usage in a week, by line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunUsage {
    /// `None` for usage outside any run.
    pub run_id: Option<String>,
    pub lines: BTreeMap<String, Totals>,
    pub total: Totals,
}

/// One person's usage in a week.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PersonUsage {
    pub person: String,
    pub total: Totals,
    /// Tokens of every kind.
    pub tokens: i64,
}

/// A week's usage report for the admin pages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsageReport {
    pub week: String,
    pub by_person: Vec<PersonUsage>,
    pub by_line: BTreeMap<String, Totals>,
    /// What every credit spent is worth at its snapshotted price.
    pub value_micro_usd: i64,
    /// Paid credits (beyond the free allowance) at their price.
    pub revenue_micro_usd: i64,
    pub cost_micro_usd: i64,
    /// The cost of the credits the free allowance covered.
    pub free_tier_cost_micro_usd: i64,
    /// The cost the instance absorbed (rows charging no credits for a
    /// model call made again), part of `cost_micro_usd`.
    pub absorbed_micro_usd: i64,
    /// (value − cost) ÷ value, as if every credit were paid; `None` with
    /// nothing spent.
    pub gross_margin_before_free_bp: Option<i64>,
    /// revenue − cost.
    pub gross_after_free_micro_usd: i64,
    /// (revenue − cost) ÷ revenue; `None` without revenue.
    pub gross_margin_after_free_bp: Option<i64>,
}

fn rows_where(
    c: &Connection,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<UsageRow>> {
    let mut q = c.prepare_cached(&format!(
        "SELECT {USAGE_COLS} FROM usage_rows WHERE {filter} ORDER BY id"
    ))?;
    let rows = q.query_map(args, row_to_usage)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `n` × 10000 ÷ `d`, rounded toward zero; `None` for `d` ≤ 0.
fn ratio_bp(n: i64, d: i64) -> Option<i64> {
    (d > 0).then(|| i64::try_from(i128::from(n) * 10_000 / i128::from(d)).unwrap_or(i64::MAX))
}

impl Store {
    /// Line `id`'s rates now with the credit price, to snapshot ([`Rate`]),
    /// or `None` for no such line.
    ///
    /// # Errors
    /// The store's.
    pub fn rate_of(&self, id: &str) -> Result<Option<Rate>> {
        let ov = self.bot_overrides();
        self.with_read(|c| {
            let price = int_setting_in(c, &ov, Setting::CreditPriceMicroUsd)?;
            Ok(line_in(c, id)?.map(|line| Rate {
                unit: line.unit,
                cost_micro_usd: line.cost_micro_usd,
                margin_bp: line.margin_bp,
                fixed_credits_milli: line.fixed_credits_milli,
                applies: line.applies,
                credit_price_micro_usd: price,
            }))
        })
    }

    /// Every cost line, by ID.
    ///
    /// # Errors
    /// The store's.
    pub fn cost_lines(&self) -> Result<Vec<CostLine>> {
        self.with_read(|c| {
            let mut q =
                c.prepare_cached(&format!("SELECT {LINE_COLS} FROM cost_lines ORDER BY id"))?;
            let rows = q.query_map([], row_to_line)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Cost line `id`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn cost_line(&self, id: &str) -> Result<Option<CostLine>> {
        self.with_read(|c| line_in(c, id))
    }

    /// Makes cost line `id` from `patch`, or changes it when it exists,
    /// recording `admin.cost_line {line, op: create|update, fields}` under
    /// `ctx` with the fields set (nothing when nothing changes). A new line
    /// needs `label`, `unit`, `cost_micro_usd`, `margin_bp`, `applies` and
    /// `source`. Changes apply to new usage only.
    ///
    /// # Errors
    /// `invalid_cost_line` for an ID, a missing field of a new line or a
    /// value out of range; the store's.
    pub fn put_cost_line(
        &self,
        ctx: &AuditCtx,
        id: &str,
        patch: CostLinePatch,
    ) -> Result<CostLine> {
        self.with_tx(|tx| self.put_line_in(tx, ctx, id, patch))
    }

    /// [`Store::put_cost_line`] on `tx`.
    fn put_line_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        id: &str,
        patch: CostLinePatch,
    ) -> Result<CostLine> {
        check_line_id(id)?;
        let label = patch
            .label
            .as_deref()
            .map(|l| check_text("label", l, 120))
            .transpose()?;
        let source = patch
            .source
            .as_deref()
            .map(|s| check_text("source", s, 500))
            .transpose()?;
        let model = match &patch.model {
            Some(Some(m)) => Some(Some(check_text("model", m, 128)?)),
            other => other.clone(),
        };
        let cost = patch
            .cost_micro_usd
            .map(|v| check_amount("cost", v, MAX_COST_MICRO_USD))
            .transpose()?;
        let margin = patch
            .margin_bp
            .map(|v| check_amount("margin", v, MAX_MARGIN_BP))
            .transpose()?;
        let fixed = match patch.fixed_credits_milli {
            Some(Some(v)) => Some(Some(check_amount(
                "fixed credits",
                v,
                MAX_FIXED_CREDITS_MILLI,
            )?)),
            other => other,
        };
        {
            let now = self.now();
            let before = line_in(tx, id)?;
            let missing = |what: &str| {
                CoreError::invalid(
                    "invalid_cost_line",
                    format!("a new cost line needs its {what}"),
                )
            };
            let after = match &before {
                Some(b) => CostLine {
                    label: label.clone().unwrap_or_else(|| b.label.clone()),
                    unit: patch.unit.unwrap_or(b.unit),
                    cost_micro_usd: cost.unwrap_or(b.cost_micro_usd),
                    margin_bp: margin.unwrap_or(b.margin_bp),
                    fixed_credits_milli: fixed.unwrap_or(b.fixed_credits_milli),
                    applies: patch.applies.unwrap_or(b.applies),
                    source: source.clone().unwrap_or_else(|| b.source.clone()),
                    model: model.clone().unwrap_or_else(|| b.model.clone()),
                    updated_by: set_by(ctx),
                    updated_at: now.clone(),
                    id: id.to_string(),
                },
                None => CostLine {
                    id: id.to_string(),
                    label: label.clone().ok_or_else(|| missing("label"))?,
                    unit: patch.unit.ok_or_else(|| missing("unit"))?,
                    cost_micro_usd: cost.ok_or_else(|| missing("cost"))?,
                    margin_bp: margin.ok_or_else(|| missing("margin"))?,
                    fixed_credits_milli: fixed.flatten(),
                    applies: patch.applies.ok_or_else(|| missing("applies"))?,
                    source: source.clone().ok_or_else(|| missing("source"))?,
                    model: model.clone().flatten(),
                    updated_by: set_by(ctx),
                    updated_at: now.clone(),
                },
            };
            let mut fields = serde_json::Map::new();
            let mut diff = |k: &str, a: serde_json::Value, b: serde_json::Value| {
                if before.is_none() || a != b {
                    fields.insert(k.into(), b);
                }
            };
            let b = before.clone();
            let old = |f: fn(&CostLine) -> serde_json::Value| {
                b.as_ref().map(f).unwrap_or(serde_json::Value::Null)
            };
            diff(
                "label",
                old(|l| l.label.clone().into()),
                after.label.clone().into(),
            );
            diff(
                "unit",
                old(|l| l.unit.as_str().into()),
                after.unit.as_str().into(),
            );
            diff(
                "cost_micro_usd",
                old(|l| l.cost_micro_usd.into()),
                after.cost_micro_usd.into(),
            );
            diff(
                "margin_bp",
                old(|l| l.margin_bp.into()),
                after.margin_bp.into(),
            );
            diff(
                "fixed_credits_milli",
                old(|l| l.fixed_credits_milli.into()),
                after.fixed_credits_milli.into(),
            );
            diff(
                "applies",
                old(|l| l.applies.as_str().into()),
                after.applies.as_str().into(),
            );
            diff(
                "source",
                old(|l| l.source.clone().into()),
                after.source.clone().into(),
            );
            diff(
                "model",
                old(|l| l.model.clone().into()),
                after.model.clone().into(),
            );
            if fields.is_empty() {
                return Ok(before.expect("an unchanged line exists"));
            }
            tx.execute(
                "INSERT INTO cost_lines (id, label, unit, cost_micro_usd, margin_bp,
                    fixed_credits_milli, applies, source, model, updated_by, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT (id) DO UPDATE SET label = excluded.label, unit = excluded.unit,
                   cost_micro_usd = excluded.cost_micro_usd, margin_bp = excluded.margin_bp,
                   fixed_credits_milli = excluded.fixed_credits_milli, applies = excluded.applies,
                   source = excluded.source, model = excluded.model,
                   updated_by = excluded.updated_by, updated_at = excluded.updated_at",
                params![
                    after.id,
                    after.label,
                    after.unit.as_str(),
                    after.cost_micro_usd,
                    after.margin_bp,
                    after.fixed_credits_milli,
                    after.applies.as_str(),
                    after.source,
                    after.model,
                    after.updated_by,
                    after.updated_at
                ],
            )?;
            let rec = AuditRecord::new(AuditKind::AdminCostLine, now)
                .with("line", id)
                .with("op", if before.is_some() { "update" } else { "create" })
                .with("fields", serde_json::Value::Object(fields));
            self.record_audit(tx, ctx, rec)?;
            Ok(after)
        }
    }

    /// Prices `model` (a model ID, such as a custom one an admin picked) at
    /// `prices`: makes or changes its input, cached input, cache-write and
    /// output lines (instance key only) in one transaction, each change
    /// recorded as `admin.cost_line`. A run may use the model from then on.
    ///
    /// # Errors
    /// `invalid_cost_line` for a model ID or a value; the store's.
    pub fn price_model(
        &self,
        ctx: &AuditCtx,
        model: &str,
        prices: ModelPrices,
    ) -> Result<Vec<CostLine>> {
        super::bot_settings::Setting::ModelDefault
            .check(&serde_json::Value::from(model))
            .map_err(|_| {
                CoreError::invalid(
                    "invalid_cost_line",
                    "a model ID is 1 to 128 letters, digits, '.', '_', ':' and '-'",
                )
            })?;
        self.with_tx(|tx| {
            [
                (Unit::MtokInput, "input", prices.input),
                (Unit::MtokCachedInput, "cached input", prices.cached_input),
                (Unit::MtokCacheWrite, "cache write", prices.cache_write),
                (Unit::MtokOutput, "output", prices.output),
            ]
            .into_iter()
            .map(|(unit, what, cost)| {
                self.put_line_in(
                    tx,
                    ctx,
                    &token_line(model, unit),
                    CostLinePatch {
                        label: Some(format!("{} {what} tokens", prices.name)),
                        unit: Some(unit),
                        cost_micro_usd: Some(cost),
                        margin_bp: Some(prices.margin_bp),
                        fixed_credits_milli: Some(None),
                        applies: Some(Applies::InstanceKey),
                        source: Some(prices.source.clone()),
                        model: Some(Some(model.to_string())),
                    },
                )
            })
            .collect()
        })
    }

    /// Deletes cost line `id`, recorded as `admin.cost_line {line, op:
    /// delete}`. Usage rows keep the line's ID and rates. A line a run that
    /// has not ended may charge (a token line of its model, or any line
    /// without a model) is kept.
    ///
    /// # Errors
    /// `NotFound`; `cost_line_in_use` for a line a live run may charge; the
    /// store's.
    pub fn delete_cost_line(&self, ctx: &AuditCtx, id: &str) -> Result<()> {
        self.with_tx(|tx| self.delete_line_in(tx, ctx, id))
    }

    /// Replaces the whole pricing, as the admin pages' `PUT
    /// /api/admin/pricing` sends it, in one transaction: the credit price
    /// and free tier (`settings`, as [`Store::set_bot_settings`] takes
    /// them), each of `lines` made or changed ([`Store::put_cost_line`]),
    /// and every other line deleted ([`Store::delete_cost_line`]). Every
    /// value is checked before anything is written, and a refusal of any
    /// line writes nothing.
    ///
    /// # Errors
    /// `setting_overridden`, `invalid_setting`, `invalid_cost_line`,
    /// `cost_line_in_use`; the store's.
    pub fn replace_pricing(
        &self,
        ctx: &AuditCtx,
        settings: &[(Setting, Option<serde_json::Value>)],
        lines: Vec<(String, CostLinePatch)>,
    ) -> Result<()> {
        let checked = self.checked_settings(settings)?;
        for (id, _) in &lines {
            check_line_id(id)?;
        }
        self.with_tx(|tx| {
            self.apply_settings_in(tx, ctx, &checked)?;
            let mut kept = std::collections::BTreeSet::new();
            for (id, patch) in lines {
                self.put_line_in(tx, ctx, &id, patch)?;
                kept.insert(id);
            }
            let all: Vec<String> = {
                let mut q = tx.prepare_cached("SELECT id FROM cost_lines ORDER BY id")?;
                let rows = q.query_map([], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for id in all.iter().filter(|id| !kept.contains(*id)) {
                self.delete_line_in(tx, ctx, id)?;
            }
            Ok(())
        })
    }

    /// [`Store::delete_cost_line`] on `tx`.
    fn delete_line_in(&self, tx: &Transaction<'_>, ctx: &AuditCtx, id: &str) -> Result<()> {
        {
            let line = line_in(tx, id)?.ok_or(CoreError::NotFound)?;
            let in_use: i64 = tx.query_row(
                "SELECT count(*) FROM bot_runs WHERE status IN ('queued', 'running', 'stopping')
                   AND (?1 IS NULL OR model = ?1)",
                params![line.model],
                |r| r.get(0),
            )?;
            if in_use > 0 {
                return Err(CoreError::invalid(
                    "cost_line_in_use",
                    format!(
                        "cost line {id} prices runs that have not ended; change it, or delete it once they end"
                    ),
                ));
            }
            if tx.execute("DELETE FROM cost_lines WHERE id = ?1", params![id])? == 0 {
                return Err(CoreError::NotFound);
            }
            let rec = AuditRecord::new(AuditKind::AdminCostLine, self.now())
                .with("line", id)
                .with("op", "delete");
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        }
    }

    /// `person`'s lift: milli-credits added to the free tier each week,
    /// and the reason given; `(0, None)` without one.
    ///
    /// # Errors
    /// The store's.
    pub fn lift_of(&self, person: &str) -> Result<(i64, Option<String>)> {
        self.with_read(|c| {
            Ok(
                c.prepare_cached("SELECT lift_milli, reason FROM credit_lifts WHERE person = ?1")?
                    .query_row(params![person], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?
                    .unwrap_or((0, None)),
            )
        })
    }

    /// Sets `person`'s lift to `lift_milli`, which holds until changed,
    /// recording `admin.credits {person, lift_from, lift_to, reason}`
    /// under `ctx` when it changes. Returns whether it did.
    ///
    /// # Errors
    /// `invalid_lift` for a negative lift or a reason over 500 characters;
    /// `NotFound` for no live person; the store's.
    pub fn set_lift(
        &self,
        ctx: &AuditCtx,
        person: &str,
        lift_milli: i64,
        reason: Option<&str>,
    ) -> Result<bool> {
        if !(0..=1_000_000_000_000).contains(&lift_milli) {
            return Err(CoreError::invalid(
                "invalid_lift",
                "a lift is a whole number from 0",
            ));
        }
        let reason = reason.map(str::trim).filter(|r| !r.is_empty());
        if reason.is_some_and(|r| r.chars().count() > 500) {
            return Err(CoreError::invalid(
                "invalid_lift",
                "a lift's reason is at most 500 characters",
            ));
        }
        self.with_tx(|tx| {
            check_live_person(tx, person)?;
            let from = lift_in(tx, person)?;
            if from == lift_milli {
                return Ok(false);
            }
            let now = self.now();
            tx.execute(
                "INSERT INTO credit_lifts (person, lift_milli, reason, set_by, set_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (person) DO UPDATE SET lift_milli = excluded.lift_milli,
                   reason = excluded.reason, set_by = excluded.set_by, set_at = excluded.set_at",
                params![person, lift_milli, reason, set_by(ctx), now],
            )?;
            let rec = AuditRecord::new(AuditKind::AdminCredits, now)
                .with("person", person)
                .with("lift_from", from)
                .with("lift_to", lift_milli)
                .with("reason", reason);
            self.record_audit(tx, ctx, rec)?;
            Ok(true)
        })
    }

    /// `person`'s credits for the week `at` falls in.
    ///
    /// # Errors
    /// The store's.
    pub fn week_usage(&self, person: &str, at: DateTime<Utc>) -> Result<WeekUsage> {
        let ov = self.bot_overrides();
        self.with_read(|c| week_usage_in(c, &ov, person, at))
    }

    /// [`Store::week_usage`] now, by the store's clock.
    ///
    /// # Errors
    /// The store's.
    pub fn week_usage_now(&self, person: &str) -> Result<WeekUsage> {
        self.week_usage(person, self.now_utc())
    }

    /// Charges `items` to `person` now, outside any step, for the tests
    /// alone: usage is charged only with a step's commit
    /// ([`Store::commit_bot_step`]), which records its events.
    #[cfg(test)]
    pub(crate) fn charge(
        &self,
        person: &str,
        run: Option<(&str, i64)>,
        items: &[Metered],
    ) -> Result<Vec<UsageRow>> {
        let at = self.now_utc();
        let ov = self.bot_overrides();
        self.with_tx(|tx| charge_in(tx, &ov, person, run, items, at))
    }

    /// `person`'s usage rows in the week `at` falls in, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn usage_rows(&self, person: &str, at: DateTime<Utc>) -> Result<Vec<UsageRow>> {
        let week = week_key(at);
        self.with_read(|c| rows_where(c, "person = ?1 AND week = ?2", &[&person, &week]))
    }

    /// Run `run`'s usage rows, oldest first.
    ///
    /// # Errors
    /// The store's.
    pub fn usage_of_run(&self, run: &str) -> Result<Vec<UsageRow>> {
        self.with_read(|c| rows_where(c, "run_id = ?1", &[&run]))
    }

    /// `person`'s usage in the week `at` falls in, by run (oldest run
    /// first; usage outside a run last) and line.
    ///
    /// # Errors
    /// The store's.
    pub fn usage_by_run(&self, person: &str, at: DateTime<Utc>) -> Result<Vec<RunUsage>> {
        let rows = self.usage_rows(person, at)?;
        let mut out: Vec<RunUsage> = Vec::new();
        for r in &rows {
            let i = match out.iter().position(|u| u.run_id == r.run_id) {
                Some(i) => i,
                None => {
                    out.push(RunUsage {
                        run_id: r.run_id.clone(),
                        lines: BTreeMap::new(),
                        total: Totals::default(),
                    });
                    out.len() - 1
                }
            };
            out[i].lines.entry(r.line.clone()).or_default().add(r);
            out[i].total.add(r);
        }
        out.sort_by_key(|u| u.run_id.is_none());
        Ok(out)
    }

    /// The usage report for the week `at` falls in: totals by person
    /// (most credits first) and by line, what the credits are worth, the
    /// revenue (paid credits at their price), the cost, the free tier's
    /// cost and the gross margin before and after the free tier. Integer
    /// arithmetic throughout, each row's share rounded toward zero.
    ///
    /// # Errors
    /// The store's.
    pub fn usage_report(&self, at: DateTime<Utc>) -> Result<UsageReport> {
        let week = week_key(at);
        let rows = self.with_read(|c| rows_where(c, "week = ?1", &[&week]))?;
        let mut people: BTreeMap<String, PersonUsage> = BTreeMap::new();
        let mut by_line: BTreeMap<String, Totals> = BTreeMap::new();
        let (mut value, mut revenue, mut cost, mut free_cost) = (0i64, 0i64, 0i64, 0i64);
        let mut absorbed = 0i64;
        for r in &rows {
            let p = people
                .entry(r.person.clone())
                .or_insert_with(|| PersonUsage {
                    person: r.person.clone(),
                    total: Totals::default(),
                    tokens: 0,
                });
            p.total.add(r);
            if r.unit.is_tokens() {
                p.tokens += r.qty;
            }
            by_line.entry(r.line.clone()).or_default().add(r);
            let at_price = |milli: i64| {
                i64::try_from(i128::from(milli) * i128::from(r.credit_price_micro_usd) / 1000)
                    .unwrap_or(i64::MAX)
            };
            value += at_price(r.credits_milli);
            revenue += at_price(r.credits_milli - r.free_milli);
            cost += r.cost_micro_usd;
            if r.absorbed {
                absorbed += r.cost_micro_usd;
            }
            if r.credits_milli > 0 {
                free_cost += i64::try_from(
                    i128::from(r.cost_micro_usd) * i128::from(r.free_milli)
                        / i128::from(r.credits_milli),
                )
                .unwrap_or(i64::MAX);
            }
        }
        let mut by_person: Vec<PersonUsage> = people.into_values().collect();
        by_person.sort_by(|a, b| {
            b.total
                .credits_milli
                .cmp(&a.total.credits_milli)
                .then_with(|| a.person.cmp(&b.person))
        });
        Ok(UsageReport {
            week,
            by_person,
            by_line,
            value_micro_usd: value,
            revenue_micro_usd: revenue,
            cost_micro_usd: cost,
            free_tier_cost_micro_usd: free_cost,
            absorbed_micro_usd: absorbed,
            gross_margin_before_free_bp: ratio_bp(value - cost, value),
            gross_after_free_micro_usd: revenue - cost,
            gross_margin_after_free_bp: ratio_bp(revenue - cost, revenue),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_settings::Setting;
    use crate::roles::OWNER;
    use crate::roles::tests::person;
    use crate::test_util::{DAEMON, store, store_with_clock};
    use clax_core::working::ManualClock;
    use serde_json::json;
    use std::sync::Arc;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn weeks_start_on_monday_at_midnight_utc() {
        // 2026-10-25 is a Sunday, 2026-10-26 a Monday.
        assert_eq!(
            week_key(t("2026-10-25T23:59:59.999Z")),
            "2026-10-19T00:00:00Z"
        );
        assert_eq!(week_key(t("2026-10-26T00:00:00Z")), "2026-10-26T00:00:00Z");
        assert_eq!(week_key(t("2026-10-19T00:00:00Z")), "2026-10-19T00:00:00Z");
        // A time in another offset is placed by its UTC instant.
        let late = DateTime::parse_from_rfc3339("2026-10-25T20:00:00-05:00").unwrap();
        assert_eq!(week_key(late.with_timezone(&Utc)), "2026-10-26T00:00:00Z");
        assert_eq!(
            week_end(t("2026-10-21T12:00:00Z")),
            t("2026-10-26T00:00:00Z")
        );
    }

    #[test]
    fn credits_per_unit_round_up_to_the_milli_credit() {
        // $2 per Mtok at 40 % at $0.01 a credit: 280 credits exactly.
        assert_eq!(credits_per_unit_milli(2_000_000, 4_000, 10_000), 280_000);
        // 17 µ$ at 90 % = 32.3 µ$, which at 10 000 µ$ a credit is 3.23
        // milli-credits: rounded up to 4.
        assert_eq!(credits_per_unit_milli(17, 9_000, 10_000), 4);
        // 1 µ$ at 0 % at 10 000 µ$ is 0.1 milli-credit: rounded up to 1.
        assert_eq!(credits_per_unit_milli(1, 0, 10_000), 1);
        // 7 µ$ at 33.33 % (3 333 bp) at 3 µ$ a credit: 3 111.03… rounds up.
        assert_eq!(credits_per_unit_milli(7, 3_333, 3), 3_112);
        assert_eq!(credits_per_unit_milli(0, 9_000, 10_000), 0);
        // Rows round up too: 1 token of a 280-credit Mtok is 0.28
        // milli-credits, charged 1.
        assert_eq!(scaled(1, 280_000, 1_000_000).unwrap(), 1);
        assert_eq!(scaled(1_000_000, 280_000, 1_000_000).unwrap(), 280_000);
        assert_eq!(scaled(1_500_001, 280_000, 1_000_000).unwrap(), 420_001);
    }

    #[test]
    fn a_row_is_rounded_once_from_the_exact_rate() {
        // 128 000 MiB·ms of compute at 17 µ$ per GiB-s at 90 % at $0.01 a
        // credit: 0.40375 milli-credits exactly, charged 1. 125 GiB-s in one
        // row is 403.75, charged 404; rounding the rate first (3.23 → 4 a
        // GiB-s) would charge 500, 24 % more.
        assert_eq!(
            row_credits_milli(128_000, 17, 9_000, None, 10_000, 1_024_000).unwrap(),
            1
        );
        assert_eq!(
            row_credits_milli(128_000_000, 17, 9_000, None, 10_000, 1_024_000).unwrap(),
            404
        );
        assert_eq!(credits_per_unit_milli(17, 9_000, 10_000) * 125, 500);
        // A fixed line charges qty × fixed ÷ base.
        assert_eq!(
            row_credits_milli(1, 0, 9_000, Some(1_000), 10_000, 1).unwrap(),
            1_000
        );
        assert_eq!(
            row_credits_milli(0, 17, 9_000, None, 10_000, 1_024_000).unwrap(),
            0
        );
        assert_eq!(
            row_credits_milli(1_000_000, 2_000_000, 4_000, None, 10_000, 1_000_000).unwrap(),
            280_000
        );
        // The worst case the bounds allow: the largest quantity at the
        // highest cost and margin and a price of 1 µ$ computes in i128 and
        // is refused for leaving i64, never wrapped.
        for fixed in [None, Some(MAX_FIXED_CREDITS_MILLI)] {
            assert!(matches!(
                row_credits_milli(i64::MAX, MAX_COST_MICRO_USD, MAX_MARGIN_BP, fixed, 1, 1),
                Err(CoreError::Invalid {
                    code: "charge_overflow",
                    ..
                })
            ));
        }
        let worst = i128::from(i64::MAX)
            * i128::from(MAX_COST_MICRO_USD)
            * i128::from(10_000 + MAX_MARGIN_BP)
            * 1000;
        assert!(worst < i128::MAX, "the bounds keep the product in i128");
        assert!(scaled(i64::MAX, MAX_COST_MICRO_USD, 1).is_err());
        // Out-of-bounds figures are refused at the line.
        let (_d, st) = store();
        for patch in [
            CostLinePatch {
                margin_bp: Some(MAX_MARGIN_BP + 1),
                ..Default::default()
            },
            CostLinePatch {
                cost_micro_usd: Some(MAX_COST_MICRO_USD + 1),
                ..Default::default()
            },
            CostLinePatch {
                fixed_credits_milli: Some(Some(MAX_FIXED_CREDITS_MILLI + 1)),
                ..Default::default()
            },
        ] {
            assert!(
                st.put_cost_line(DAEMON, LINE_SCRIPT_COMPUTE, patch)
                    .is_err()
            );
        }
        // A charge that would overflow is refused and writes nothing.
        assert!(matches!(
            st.charge(
                OWNER,
                None,
                &[Metered::new(
                    LINE_HARNESS_FEE,
                    i64::MAX,
                    KeySource::Instance
                )]
            ),
            Err(CoreError::Invalid {
                code: "charge_overflow",
                ..
            })
        ));
        assert!(st.usage_rows(OWNER, st.now_utc()).unwrap().is_empty());
    }

    #[test]
    fn a_custom_model_is_priced_and_a_line_in_use_is_kept() {
        let (_d, st) = store();
        let model = "my-proxy:Sonnet-X";
        assert!(matches!(
            st.with_read(|c| check_model_priced(c, model)),
            Err(CoreError::Invalid {
                code: "model_not_priced",
                ..
            })
        ));
        let lines = st
            .price_model(
                DAEMON,
                model,
                ModelPrices {
                    name: "Sonnet X".into(),
                    input: 1_000_000,
                    cached_input: 100_000,
                    cache_write: 1_250_000,
                    output: 5_000_000,
                    margin_bp: TOKEN_MARGIN_BP,
                    source: "the proxy's price list".into(),
                },
            )
            .unwrap();
        assert_eq!(lines.len(), 4);
        st.with_read(|c| check_model_priced(c, model)).unwrap();
        assert!(
            st.price_model(
                DAEMON,
                "bad model",
                ModelPrices {
                    name: "x".into(),
                    input: 1,
                    cached_input: 1,
                    cache_write: 1,
                    output: 1,
                    margin_bp: 0,
                    source: "x".into(),
                }
            )
            .is_err()
        );
        // A live run on the model keeps its lines, and every line without a
        // model; another model's lines may go.
        let aid = crate::test_util::artifact(&st, None).to_string();
        st.create_run(
            DAEMON,
            crate::bot_runs::NewRun {
                artifact_id: aid,
                person: OWNER.into(),
                request: crate::bot_runs::RequestKind::Thread,
                thread_ids: vec![],
                model: model.into(),
                key_source: KeySource::Instance,
                base_version: 1,
                agent_grant: None,
            },
        )
        .unwrap();
        for id in [
            token_line(model, Unit::MtokOutput),
            LINE_SCRIPT_COMPUTE.to_string(),
        ] {
            assert!(matches!(
                st.delete_cost_line(DAEMON, &id),
                Err(CoreError::Invalid {
                    code: "cost_line_in_use",
                    ..
                })
            ));
        }
        st.delete_cost_line(DAEMON, &token_line("claude-opus-5-5", Unit::MtokOutput))
            .unwrap();
    }

    #[test]
    fn migration_seeds_every_model_and_infrastructure_line() {
        let (_d, st) = store();
        let lines = st.cost_lines().unwrap();
        for (model, _, input, cached, output) in SEED_MODELS {
            for (unit, cost) in [
                (Unit::MtokInput, input),
                (Unit::MtokCachedInput, cached),
                (Unit::MtokCacheWrite, cache_write_price(input)),
                (Unit::MtokOutput, output),
            ] {
                let l = lines
                    .iter()
                    .find(|l| l.id == token_line(model, unit))
                    .unwrap();
                assert_eq!((l.unit, l.cost_micro_usd), (unit, cost));
                assert_eq!(l.margin_bp, TOKEN_MARGIN_BP);
                assert_eq!(l.applies, Applies::InstanceKey);
                assert_eq!(l.model.as_deref(), Some(model));
            }
        }
        let fee = st.cost_line(LINE_HARNESS_FEE).unwrap().unwrap();
        assert_eq!(
            (fee.cost_micro_usd, fee.fixed_credits_milli),
            (0, Some(1_000))
        );
        assert_eq!(fee.credits_per_unit_milli(10_000), 1_000);
        for id in [LINE_SCRIPT_COMPUTE, LINE_DO_REQUESTS, LINE_TOOLPATH_STORAGE] {
            let l = st.cost_line(id).unwrap().unwrap();
            assert_eq!((l.margin_bp, l.applies), (INFRA_MARGIN_BP, Applies::Always));
        }
        assert_eq!(lines.len(), 4 * 4 + 4);
        assert_eq!(
            st.cost_line("claude-sonnet-5-5.cache_write")
                .unwrap()
                .unwrap()
                .cost_micro_usd,
            2_500_000,
            "1.25 times input"
        );
        for (model, ..) in SEED_MODELS {
            st.with_read(|c| check_model_priced(c, model)).unwrap();
        }
    }

    #[test]
    fn a_charge_snapshots_its_rates_and_history_never_reprices() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let line = token_line("claude-sonnet-5-5", Unit::MtokInput);
        let rows = st
            .charge(
                OWNER,
                Some(("r1", 1)),
                &[Metered::new(&line, 1_000_000, KeySource::Instance)],
            )
            .unwrap();
        assert_eq!(rows[0].credits_milli, 280_000);
        assert_eq!(rows[0].cost_micro_usd, 2_000_000);
        assert_eq!(rows[0].rate_cost_micro_usd, 2_000_000);
        assert_eq!(rows[0].margin_bp, 4_000);
        assert_eq!(rows[0].base_qty, 1_000_000);
        assert_eq!(rows[0].fixed_credits_milli, None);
        assert_eq!(rows[0].credit_price_micro_usd, 10_000);
        assert_eq!(rows[0].week, "2026-10-19T00:00:00Z");
        // The price and the line change; the row stays.
        st.put_cost_line(
            DAEMON,
            &line,
            CostLinePatch {
                margin_bp: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        st.set_bot_settings(
            DAEMON,
            &[(Setting::CreditPriceMicroUsd, Some(json!(20_000)))],
        )
        .unwrap();
        let kept = st.usage_rows(OWNER, st.now_utc()).unwrap();
        assert_eq!(kept, rows);
        let new = st
            .charge(
                OWNER,
                None,
                &[Metered::new(&line, 1_000_000, KeySource::Instance)],
            )
            .unwrap();
        assert_eq!(new[0].credits_milli, 100_000, "$2 at 0 % at $0.02");
        assert_eq!(new[0].margin_bp, 0);
    }

    #[test]
    fn own_key_rows_are_free_and_always_lines_still_charge() {
        let (_d, st) = store();
        let rows = st
            .charge(
                OWNER,
                Some(("r1", 1)),
                &[
                    Metered::new(
                        token_line("claude-opus-5-5", Unit::MtokOutput),
                        5_000,
                        KeySource::Own,
                    ),
                    Metered::new(LINE_HARNESS_FEE, 1, KeySource::Own),
                    Metered::new(LINE_SCRIPT_COMPUTE, 64 * 2_000, KeySource::Own),
                ],
            )
            .unwrap();
        assert_eq!((rows[0].cost_micro_usd, rows[0].credits_milli), (0, 0));
        assert_eq!(rows[0].qty, 5_000, "visible, not charged");
        assert_eq!(rows[0].rate_cost_micro_usd, 20_000_000);
        assert_eq!(rows[1].credits_milli, 1_000);
        // 128 000 MiB·ms at 17 µ$ a GiB-s at 90 %: 0.40375 → 1.
        assert_eq!(rows[2].credits_milli, 1);
        assert_eq!(rows[2].cost_micro_usd, 3, "17 × 0.125 = 2.125 → 3");
        assert!(matches!(
            st.charge(OWNER, None, &[Metered::new("nope", 1, KeySource::Instance)]),
            Err(CoreError::Invalid {
                code: "unknown_cost_line",
                ..
            })
        ));
        assert!(
            st.charge(
                OWNER,
                None,
                &[Metered::new(LINE_HARNESS_FEE, -1, KeySource::Own)]
            )
            .is_err()
        );
        // A refused charge writes nothing.
        assert_eq!(st.usage_rows(OWNER, st.now_utc()).unwrap().len(), 3);
    }

    #[test]
    fn the_allowance_resets_on_monday_and_lifts_add_to_it() {
        // Sunday, one second before the week ends.
        let clock = Arc::new(ManualClock::at("2026-10-25T23:59:59Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let sam = person(&st, "Sam", 1);
        st.set_bot_settings(DAEMON, &[(Setting::FreeWeeklyMilli, Some(json!(3_000)))])
            .unwrap();
        // Three fees fill the week; a fourth goes past it.
        for _ in 0..4 {
            st.charge(
                &sam,
                None,
                &[Metered::new(LINE_HARNESS_FEE, 1, KeySource::Instance)],
            )
            .unwrap();
        }
        let w = st.week_usage_now(&sam).unwrap();
        assert_eq!(w.start, "2026-10-19T00:00:00Z");
        assert_eq!(w.end, "2026-10-26T00:00:00Z");
        assert_eq!((w.weekly_milli, w.spent_milli), (3_000, 4_000));
        assert!(w.warn && w.exhausted);
        assert_eq!(w.remaining_milli(), 0);
        let rows = st.usage_rows(&sam, st.now_utc()).unwrap();
        let free: Vec<i64> = rows.iter().map(|r| r.free_milli).collect();
        assert_eq!(free, [1_000, 1_000, 1_000, 0]);
        assert!(rows[0].free() && !rows[3].free());

        // Monday 00:00 UTC: a new week.
        clock.advance(1);
        let w = st.week_usage_now(&sam).unwrap();
        assert_eq!(w.start, "2026-10-26T00:00:00Z");
        assert_eq!((w.spent_milli, w.warn, w.exhausted), (0, false, false));

        // A lift holds until changed, and is recorded.
        assert!(
            st.set_lift(DAEMON, &sam, 2_000, Some("launch week"))
                .unwrap()
        );
        assert!(!st.set_lift(DAEMON, &sam, 2_000, None).unwrap());
        assert_eq!(
            st.lift_of(&sam).unwrap(),
            (2_000, Some("launch week".into()))
        );
        let w = st.week_usage_now(&sam).unwrap();
        assert_eq!((w.weekly_milli, w.lift_milli), (5_000, 2_000));
        for _ in 0..4 {
            st.charge(
                &sam,
                None,
                &[Metered::new(LINE_HARNESS_FEE, 1, KeySource::Instance)],
            )
            .unwrap();
        }
        let w = st.week_usage_now(&sam).unwrap();
        assert!(w.warn && !w.exhausted, "{w:?}");
        let ev = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == "admin.credits")
            .unwrap();
        assert!(ev.body.contains("\"lift_from\":0") && ev.body.contains("\"lift_to\":2000"));
        assert!(st.set_lift(DAEMON, &sam, -1, None).is_err());
        assert!(matches!(
            st.set_lift(DAEMON, "u_nobody", 1, None),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn cost_lines_are_made_changed_and_deleted_with_their_fields_recorded() {
        let (_d, st) = store();
        let missing = st.put_cost_line(
            DAEMON,
            "gpu.time",
            CostLinePatch {
                label: Some("GPU".into()),
                ..Default::default()
            },
        );
        assert!(matches!(
            missing,
            Err(CoreError::Invalid {
                code: "invalid_cost_line",
                ..
            })
        ));
        assert!(
            st.put_cost_line(DAEMON, "Bad ID", CostLinePatch::default())
                .is_err()
        );
        let made = st
            .put_cost_line(
                DAEMON,
                "gpu.time",
                CostLinePatch {
                    label: Some("GPU".into()),
                    unit: Some(Unit::CpuGbS),
                    cost_micro_usd: Some(100),
                    margin_bp: Some(5_000),
                    applies: Some(Applies::Always),
                    source: Some("a quote".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(made.credits_per_unit_milli(10_000), 15);
        let same = st
            .put_cost_line(
                DAEMON,
                "gpu.time",
                CostLinePatch {
                    margin_bp: Some(5_000),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(same, made, "no change, nothing recorded");
        st.put_cost_line(
            DAEMON,
            "gpu.time",
            CostLinePatch {
                cost_micro_usd: Some(200),
                ..Default::default()
            },
        )
        .unwrap();
        st.delete_cost_line(DAEMON, "gpu.time").unwrap();
        assert!(matches!(
            st.delete_cost_line(DAEMON, "gpu.time"),
            Err(CoreError::NotFound)
        ));
        let bodies: Vec<serde_json::Value> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "admin.cost_line")
            .map(|e| serde_json::from_str(&e.body).unwrap())
            .collect();
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[0]["op"], "create");
        assert_eq!(bodies[0]["fields"]["margin_bp"], 5_000);
        assert_eq!(bodies[1]["op"], "update");
        assert_eq!(bodies[1]["fields"], json!({"cost_micro_usd": 200}));
        assert_eq!(bodies[2]["op"], "delete");
    }

    #[test]
    fn the_report_splits_revenue_cost_and_the_free_tier() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        st.set_bot_settings(DAEMON, &[(Setting::FreeWeeklyMilli, Some(json!(140_000)))])
            .unwrap();
        st.set_flags(
            DAEMON,
            &sam,
            &[(crate::flags::Flag::ClaxbotUnenforced, true)],
        )
        .unwrap();
        // 1 Mtok of Sonnet input: 280 credits, $2 cost; half of it free.
        let line = token_line("claude-sonnet-5-5", Unit::MtokInput);
        st.charge(
            &sam,
            Some(("r1", 1)),
            &[Metered::new(&line, 1_000_000, KeySource::Instance)],
        )
        .unwrap();
        st.charge(
            OWNER,
            Some(("r2", 1)),
            &[Metered::new(LINE_HARNESS_FEE, 1, KeySource::Own)],
        )
        .unwrap();
        let r = st.usage_report(st.now_utc()).unwrap();
        assert_eq!(r.by_person[0].person, sam);
        assert_eq!(r.by_person[0].tokens, 1_000_000);
        assert_eq!(r.by_person[0].total.credits_milli, 280_000);
        assert_eq!(r.by_person[0].total.free_milli, 140_000);
        assert_eq!(r.by_line[&line].qty, 1_000_000);
        // Worth 281 credits at $0.01; Sam's 140 paid credits are $1.40.
        assert_eq!(r.value_micro_usd, 2_810_000);
        assert_eq!(r.revenue_micro_usd, 1_400_000);
        assert_eq!(r.cost_micro_usd, 2_000_000);
        assert_eq!(r.free_tier_cost_micro_usd, 1_000_000);
        assert_eq!(r.gross_margin_before_free_bp, Some(2_882));
        assert_eq!(r.gross_after_free_micro_usd, -600_000);
        assert_eq!(r.gross_margin_after_free_bp, Some(-4_285));

        let runs = st.usage_by_run(&sam, st.now_utc()).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id.as_deref(), Some("r1"));
        assert_eq!(runs[0].total.credits_milli, 280_000);

        let empty = st.usage_report(st.now_utc() + Duration::days(7)).unwrap();
        assert_eq!(empty.gross_margin_before_free_bp, None);
        assert_eq!(empty.gross_margin_after_free_bp, None);
    }
}
