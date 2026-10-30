// ClaxBot fixtures for the unit tests: the shapes of spec 2026-10-21-claxbot
// §10 with plausible values, each overridable.
import type { AdminPerson, AdminUsage, BotRun, MeBot, Pricing, Provider, Settings, Week } from "./api";

export const WEEK_START = "2026-10-19T00:00:00Z";
export const WEEK_END = "2026-10-26T00:00:00Z";
export const week = (o: Partial<Week> = {}): Week => ({ start: WEEK_START, end: WEEK_END, weekly_milli: 2_000_000, lift_milli: 0, spent_milli: 310_000, warn: false, exhausted: false, ...o });
export const meBot = (o: Partial<MeBot> = {}): MeBot => ({
  enabled: true, unenforced: false, available: true, unavailable_reason: null,
  key: { own: null, use_own: false }, week: week(), role: "member", ...o,
});
export const adminPerson = (o: Partial<AdminPerson> = {}): AdminPerson => ({
  person: "p_sam", name: "Sam", role: "member", flags: { claxbot: true, claxbot_unenforced: false }, key_source: "own",
  week: { weekly_milli: 2_500_000, lift_milli: 500_000, spent_milli: 440_000 }, tokens_week: 1_300_000, runs_week: 4, refused_week: 1, ...o,
});
export const provider = (o: Partial<Provider> = {}): Provider => ({
  provider: "anthropic", base_url: "https://api.anthropic.com", models: { default: "claude-sonnet-5-5", quick: "claude-haiku-5-5", complex: "claude-opus-5-5" },
  key: { set: true, last4: "77d1", set_at: "2026-10-21T09:00:00Z", set_by: { id: "*owner", name: "Alex" }, last_ok_at: "2026-10-21T11:58:00Z" },
  caps: { max_steps: 40, max_run_credits: 200 }, sources: { base_url: "default", key: "db", "models.default": "db", "models.quick": "default", "models.complex": "default", "caps.max_steps": "file", "caps.max_run_credits": "default" }, ...o,
});
export const pricing = (o: Partial<Pricing> = {}): Pricing => ({
  credit_price_micro_usd: 10_000, free_weekly_milli: 2_000_000,
  lines: [
    { id: "sonnet-in", label: "Sonnet 5.5 input", unit: "mtok_input", cost_micro_usd: 3_000_000, margin_bp: 4000, applies: "instance_key", source: "Anthropic list price", model: "claude-sonnet-5-5" },
    { id: "compute", label: "Script compute", unit: "cpu_gb_s", cost_micro_usd: 4000, margin_bp: 9000, applies: "always", source: "Cloudflare" },
    { id: "harness", label: "Harness fee", unit: "run", cost_micro_usd: 0, margin_bp: 0, applies: "always", source: "fixed", fixed_credits_milli: 1000 },
  ], ...o,
});
export const usage = (o: Partial<AdminUsage> = {}): AdminUsage => ({
  week: { start: WEEK_START, end: WEEK_END }, paid_milli: 3_410_000, free_milli: 710_000, runs: 186, refused: 4,
  revenue_micro_usd: 34_100_000, cost_micro_usd: 22_900_000, free_cost_micro_usd: 4_600_000,
  by_person: [{ person: "p_sam", name: "Sam", credits_milli: 440_000, free_milli: 440_000, cost_micro_usd: 3_000_000, runs: 4, tokens: 1_300_000 }],
  by_line: [{ line: "compute", label: "Script compute", unit: "cpu_gb_s", qty: 412_000, credits_milli: 310_000, cost_micro_usd: 1_600_000 }], ...o,
});
/** As `GET /api/settings` answers (crates/clax-server/src/routes/settings.rs). */
export const settings = (o: Partial<Settings> = {}): Settings => ({
  file: "/home/alex/.clax/config.toml",
  fields: [
    { key: "serve.port", value: 7480, source: "env:CLAX_SERVE__PORT", editable: false, restart_required: true, env: "CLAX_SERVE__PORT" },
    { key: "bot.enabled", value: true, source: "file", editable: true, restart_required: true, env: "CLAX_BOT__ENABLED" },
    { key: "bot.base_url", value: "https://api.anthropic.com", source: "default", editable: true, restart_required: true, env: "CLAX_BOT__BASE_URL" },
    { key: "bot.api_key_env", value: "ANTHROPIC_API_KEY", source: "default", pending: "CLAX_KEY", editable: true, restart_required: true, env: "CLAX_BOT__API_KEY_ENV" },
    { key: "bot.models.default", value: "claude-sonnet-5-5", source: "default", editable: true, restart_required: true, env: "CLAX_BOT__MODELS__DEFAULT" },
  ],
  problems: [], started_with_invalid_config: false, start_error: null, ...o,
});
export const run = (o: Partial<BotRun> = {}): BotRun => ({
  id: "r1", artifact_id: "7q3k9mzx2b4t", thread_ids: ["t1"], request: "thread", status: "running", model: "claude-sonnet-5-5", key_source: "own",
  acting_for: { id: "p_sam", name: "Sam" }, started_at: "2026-10-21T14:02:00Z", credits_milli: 2000, tokens: { input: 30_000, cached: 6_000, output: 2_200 },
  steps: [], ...o,
});
