// ClaxBot's numbers and words for the shell (spec 2026-10-21-claxbot §7,
// §11): credits from milli-credits, money from micro-USD, margins from
// basis points; the week's meter and its warn and exhausted states; the
// Send menu's availability line; a cost line's credits per unit; where a
// model's token rates come from and its price in a line; the margin
// before and after the free tier; a run's state as its events arrive; the
// audit's time ranges; and the settings' sections. Pure: no clock but the
// one passed in.
import type { AdminUsage, BotCall, BotRun, BotStep, CostLine, ErrorDetail, KeySource, MeBot, PriceSummary, RateSource, RunEvent, RunStatus, SettingField, Source, TokenUnit, Unavailable, Unit, Week } from "./api";

/** A setting as its section lists it: its dotted name and the rest of it after the section. */
export type Setting = SettingField & { key: string; label: string };

const int = new Intl.NumberFormat("en-US");

/** Milli-credits as credits: whole numbers with separators ("1,690"),
 * a fraction to two places where there is one ("0.76"). */
export function credits(milli: number): string {
  if (milli % 1000 === 0) return int.format(milli / 1000);
  const v = milli / 1000;
  return new Intl.NumberFormat("en-US", { minimumFractionDigits: Math.abs(v) < 1 ? 2 : 0, maximumFractionDigits: 2 }).format(v);
}

/** Micro-USD as dollars: cents ("$34.10"), and four places under a cent ("$0.0040"). */
export function money(micro: number): string {
  const v = micro / 1e6;
  const small = v !== 0 && Math.abs(v) < 0.01;
  return `${v < 0 ? "-" : ""}$${new Intl.NumberFormat("en-US", { minimumFractionDigits: small ? 4 : 2, maximumFractionDigits: small ? 4 : 2 }).format(Math.abs(v))}`;
}

/** Basis points as a percentage ("40%", "12.5%"). */
export const percent = (bp: number): string => `${new Intl.NumberFormat("en-US", { maximumFractionDigits: 1 }).format(bp / 100)}%`;

/** A count of tokens, short ("38.2k", "9.4M"). */
export function tokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1e6) return `${(n / 1000).toFixed(n < 1e4 ? 1 : 0).replace(/\.0$/, "")}k`;
  return `${(n / 1e6).toFixed(1).replace(/\.0$/, "")}M`;
}

/** When the week's credits reset: the week's end as a UTC date ("Monday, Oct 27"). */
export const resetDay = (end: string): string =>
  new Date(end).toLocaleDateString("en-US", { weekday: "long", month: "short", day: "numeric", timeZone: "UTC" });

/** The week's meter. `weekly_milli` is the whole allowance (the free tier
 * and any lift); warn from 80 % (§7.4, the UI's own state, also given by
 * the daemon), exhausted once spent reaches it. Unenforced, the meter
 * shows the numbers and never the exhausted state's block. */
export type Meter = { spent: number; total: number; left: number; ratio: number; warn: boolean; exhausted: boolean };
export function meter(w: Week): Meter {
  const total = Math.max(0, w.weekly_milli);
  const spent = Math.max(0, w.spent_milli);
  const ratio = total ? spent / total : spent ? 1 : 0;
  const exhausted = w.exhausted || (total > 0 && spent >= total);
  return { spent, total, left: Math.max(0, total - spent), ratio: Math.min(1, ratio), warn: !exhausted && (w.warn || ratio >= 0.8), exhausted };
}

/** Whether this person's runs use their own model key. Their tokens are
 * then billed to that key and charge no credits; the lines charged on every
 * key (`applies: "always"`: the per-run fee, script compute, storage and
 * web calls) still spend the week's credits, and running out still pauses
 * ClaxBot. */
export const ownKey = (me: MeBot | null): boolean => !!me?.key.use_own && !!me.key.own?.set;

/** What credits pay for on a person's own key, in words. */
export const OWN_KEY_SPEND = "run fees, script compute, storage and web calls";

/** The week's meter in words for `me` (ClaxBot on): `label` for the gallery
 * header's link and its `aria-label`, `title` for its hover text and the
 * artifact view's. On their own key they say so, and what credits pay for. */
export function meterWords(me: MeBot): { own: boolean; label: string; title: string } {
  const m = meter(me.week);
  const used = `${credits(m.spent)} of ${credits(m.total)} credits used this week`;
  if (!ownKey(me)) return { own: false, label: `ClaxBot: ${used}`, title: "Your ClaxBot credits this week" };
  const title = `ClaxBot runs on your key, which pays for your tokens. Credits pay only for ${OWN_KEY_SPEND}.`;
  return { own: true, label: `ClaxBot on your key: ${used}, for ${OWN_KEY_SPEND}`, title };
}

/** The usage banner a page shows for `me`: none, a warning from 80 %, or
 * the exhausted state (not for an unenforced person, whom ClaxBot never
 * stops). On their own key it says what the credits paid for. */
export type Banner = { kind: "warn" | "exhausted"; text: string; detail: string } | null;
export function banner(me: MeBot | null): Banner {
  if (!me?.enabled) return null;
  const m = meter(me.week);
  const reset = resetDay(me.week.end);
  const own = ownKey(me) ? ` On your key, credits pay for ${OWN_KEY_SPEND}, not tokens.` : "";
  if (m.exhausted && !me.unenforced) {
    return { kind: "exhausted", text: "This week's credits are used up.", detail: `You've used ${credits(m.spent)} of ${credits(m.total)} credits. Your pages, comments and local agents keep working; ClaxBot is paused until ${reset}.${own}` };
  }
  if (m.warn || (m.exhausted && me.unenforced)) {
    return { kind: "warn", text: `You've used ${credits(m.spent)} of your ${credits(m.total)} credits this week.`, detail: (me.unenforced ? `Your limit is not enforced: ClaxBot keeps working. Resets ${reset}.` : `ClaxBot keeps working until you reach it. Resets ${reset}.`) + own };
  }
  return null;
}

/** Why ClaxBot cannot run for this person, in words. */
export function unavailableText(reason: Unavailable | null | undefined, week?: Week | null): string {
  switch (reason) {
    case "no_flag": return "Not turned on for you: an admin can turn it on";
    case "no_credits": return week ? `Weekly credits used · resets ${resetDay(week.end).replace(/^(\w{3})\w*,/, "$1")}` : "Weekly credits used";
    case "not_configured": return "Not set up on this Clax yet";
    case "model_not_priced": return "Its model has no price yet: an admin sets one under Pricing";
    default: return "Not available";
  }
}

/** What this person can do about why ClaxBot cannot run, and the page to
 * do it on: an admin is sent to the Admin tab that fixes it, anyone else
 * to You for what is theirs to change (their own key, their usage); null
 * when only an admin can help. */
export function unavailableFix(reason: Unavailable | null | undefined, admin: boolean): { text: string; href: string } | null {
  switch (reason) {
    case "not_configured": return admin ? { text: "Set a model key under Settings → Model provider", href: "/settings/provider" } : { text: "Use your own Anthropic key under Settings → Your ClaxBot", href: "/settings/claxbot#me-k" };
    case "model_not_priced": return admin ? { text: "Price it under Settings → Pricing", href: "/settings/pricing" } : null;
    case "no_flag": return admin ? { text: "Turn it on under Settings → Roles & ClaxBot access", href: "/settings/roles" } : null;
    case "no_credits": return admin ? { text: "Give a lift under Settings → Roles & ClaxBot access", href: "/settings/roles" } : { text: "See your usage under Settings → Your ClaxBot", href: "/settings/claxbot" };
    default: return null;
  }
}

/** The Send menu's line under ClaxBot: where it runs, whose key and the
 * credits left, or why it is unavailable. */
export function availability(live: boolean, reason: Unavailable | null | undefined, me: MeBot | null): string {
  if (!live) return unavailableText(reason ?? (me?.unavailable_reason ?? null), me?.week);
  if (!me) return "Runs in Clax";
  const key = ownKey(me) ? "your key" : "instance key";
  const m = meter(me.week);
  return `Runs in Clax · ${key} · ${credits(m.left)} credits left${me.unenforced ? " (not enforced)" : ""}`;
}

/** What one unit of a cost line costs in milli-credits (§7.2): its cost
 * with its margin over the credit price, rounded up; a fixed line its own. */
export function perUnitMilli(line: Pick<CostLine, "cost_micro_usd" | "margin_bp" | "fixed_credits_milli">, priceMicro: number): number {
  if (line.fixed_credits_milli !== null && line.fixed_credits_milli !== undefined) return line.fixed_credits_milli;
  if (priceMicro <= 0) return 0;
  return Math.ceil((line.cost_micro_usd * (10_000 + line.margin_bp) * 1000) / (10_000 * priceMicro));
}

/** The web cost lines' price per thousand requests in milli-credits: a search on each provider, and a fetch (`null` where the lines do not list it). */
export function webPrices(lines: Pick<CostLine, "id" | "cost_micro_usd" | "margin_bp" | "fixed_credits_milli">[], priceMicro: number): { search: Record<string, number>; fetch: number | null } {
  const out: { search: Record<string, number>; fetch: number | null } = { search: {}, fetch: null };
  for (const l of lines) {
    if (l.id === "web.fetch") out.fetch = perUnitMilli(l, priceMicro);
    else if (l.id.startsWith("web.search.")) out.search[l.id.slice("web.search.".length)] = perUnitMilli(l, priceMicro);
  }
  return out;
}

/** A fetch's source in words: the sender's comment, or the search or page whose result named the URL ("from search #2"). */
export function webSource(source: string | undefined): string | null {
  if (!source) return null;
  return source === "comment" ? "from the sender's comment" : `from ${source}`;
}

/** What a web call cost, in credits; nothing for a call with no cost (a page from the store, or a call that failed uncharged). */
export function callCost(milli: number | undefined): string | null {
  if (!milli || milli <= 0) return null;
  return `${credits(milli)} ${milli === 1000 ? "credit" : "credits"}`;
}

export const UNITS: Record<Unit, string> = {
  mtok_input: "1M input tokens", mtok_cached_input: "1M cached tokens", mtok_cache_write: "1M cache-write tokens", mtok_output: "1M output tokens",
  cpu_gb_s: "CPU·GB-s", mreq: "1M requests", gb_month: "GB-month", run: "run", kreq: "thousand requests",
};

/** The base unit a usage row counts in (§7.3), by its line's unit. */
export const BASE_UNITS: Record<Unit, string> = {
  mtok_input: "tokens", mtok_cached_input: "tokens", mtok_cache_write: "tokens", mtok_output: "tokens",
  cpu_gb_s: "CPU·MiB·ms", mreq: "requests", gb_month: "byte·s", run: "runs", kreq: "requests",
};

/** A rate card's token units, in order, with their names. */
export const TOKEN_UNITS: { id: TokenUnit; label: string }[] = [
  { id: "input", label: "Input" }, { id: "cached_input", label: "Cached input" }, { id: "cache_write", label: "Cache write" }, { id: "output", label: "Output" },
];
/** Where a model's token rate comes from, as its badge says it: any rate an admin did not set is an estimate (the store's `RateSource::is_estimated`). */
export const RATE_SOURCES: Record<RateSource, string> = { line: "set", catalog: "tokenulator.dev, estimated", default: "default rate, estimated", provider: "billed by the provider" };
export const estimated = (s: RateSource | null | undefined): boolean => s === "catalog" || s === "default";
/** A model's price in one line, input / output: "105 / 630 credits per Mtok, tokenulator.dev, estimated"; "no price" when it resolves to none. */
export function priceSummary(p: PriceSummary): string {
  const [i, o] = [p.input_credits_milli_per_mtok, p.output_credits_milli_per_mtok];
  if (!p.source || i === null || o === null) return "no price";
  const from = p.source === "mixed" ? (p.estimated ? "set in part, estimated" : "set") : RATE_SOURCES[p.source];
  return `${credits(i)} / ${credits(o)} credits per Mtok, ${from}`;
}

/** Gross margin before and after the free tier, in basis points (null without revenue). */
export function margins(u: Pick<AdminUsage, "revenue_micro_usd" | "cost_micro_usd" | "free_cost_micro_usd">): { before: number | null; after: number | null } {
  if (u.revenue_micro_usd <= 0) return { before: null, after: null };
  const r = u.revenue_micro_usd;
  return { before: Math.round(((r - u.cost_micro_usd) * 10_000) / r), after: Math.round(((r - u.cost_micro_usd - u.free_cost_micro_usd) * 10_000) / r) };
}

/** Where a value comes from, as its badge says it; and whether this screen may change it. */
export function sourceLabel(s: Source): string {
  if (s === "default") return "default";
  if (s === "db") return "set here";
  if (s === "file") return "config.toml";
  return `env ${s.slice(4)}`;
}
export const readOnlySource = (s: Source | undefined): boolean => s === "file" || !!s?.startsWith("env:");

// ---- runs ----

export const ENDED: ReadonlySet<RunStatus> = new Set(["finished", "gave_up", "stopped"]);
export const ended = (r: { status: RunStatus }): boolean => ENDED.has(r.status);

const REASONS: Record<string, string> = {
  credits_exhausted: "this week's credits ran out",
  run_cap: "it reached the most credits one run may spend",
  max_steps: "it reached the most steps one run may take",
  max_time: "it ran out of time",
  cancelled: "it was stopped",
  request_gone: "what it was asked to answer is gone",
  disabled: "ClaxBot is turned off here",
  no_flag: "ClaxBot is not turned on for you",
  not_configured: "no model key is set up",
  model_not_priced: "the model has no price set",
  key_rejected: "the model provider refused the key",
  model_error: "the model provider answered with an error",
  model_timeout: "the model took longer to answer than a run allows",
  internal_error: "Clax could not save its work",
  history_unreadable: "Clax could not read what it had done",
  policy_error: "Clax could not work out what this run may do",
};
/** What a failed model call on the instance's key is called: the
 * provider's own answer is for admins, so the run says this instead. */
const INSTANCE_MODEL_ERROR = "ClaxBot could not use this Clax's model provider";
/** Why a run stopped, in words, for its stop code (crates/clax-bot's
 * `lead` names the same codes); null for a code this shell has no words
 * for, which is then never shown as it is. A run on the instance's key
 * (`keySource`) that stopped on a model error says so in Clax's words. */
export function stopWords(reason: string | null | undefined, keySource?: KeySource): string | null {
  if (!reason) return null;
  if (reason === "model_error" && keySource === "instance") return INSTANCE_MODEL_ERROR;
  if (Object.hasOwn(REASONS, reason)) return REASONS[reason];
  if (reason.startsWith("policy_history_")) return "Clax could not put together what to send the model";
  return null;
}
/** The run's state in a few words (`key_source` says whose key a model error was on). */
export function runStatus(r: Pick<BotRun, "status" | "reason"> & { key_source?: KeySource }): string {
  switch (r.status) {
    case "queued": return "Starting…";
    case "running": return "Working";
    case "stopping": return "Stopping…";
    case "finished": return "Finished";
    case "gave_up": return "Gave up";
    case "stopped": { const w = stopWords(r.reason, r.key_source); return w ? `Stopped: ${w}` : "Stopped"; }
  }
}

/** A failed model call in words (`errorDetail`): what went wrong (`lead`),
 * what the provider said, quoted (`said`; the client's own description
 * where no provider answered), the status, type and request ID (`meta`),
 * and what to do next, with the page to do it on for an admin, or for
 * the person whose own key it was. */
export type ErrorWords = { lead: string; said: string | null; meta: string; next: { text: string; href?: string } | null };
const LEADS: Record<string, string> = {
  invalid_request: "The model provider refused the request as invalid.",
  auth: "The model provider refused the key.",
  rate_limited: "The model provider is limiting how often this key may call it.",
  overloaded: "The model provider is overloaded.",
  network: "The model provider could not be reached.",
  stream_cut: "The model provider's answer was cut off.",
  protocol: "The model provider's answer could not be read.",
  unknown_event: "The model provider's answer could not be read.",
  redirected: "The provider's base URL is not one Clax sends a key to.",
  bad_base_url: "The provider's base URL is not one Clax sends a key to.",
  api: "The model provider answered with an error.",
};
/** The classes whose `message` is this client's description of what it saw, not the provider's words. */
const OWN_WORDS: ReadonlySet<string> = new Set(["network", "stream_cut", "protocol", "unknown_event"]);
/** The classes that pass with time; every other one needs the key or the model looked at. */
const PASSING: ReadonlySet<string> = new Set(["rate_limited", "overloaded", "network", "stream_cut"]);
/** A failed model call in words, by its class (any class this shell does
 * not know reads as `api`) and, for `api`, its type or status. The
 * provider's message is untrusted text, shown as it is and never read to
 * decide anything. What to do next, for a class that does not pass with
 * time: on the person's own key (`keySource` `own`, admin or not), check
 * it under Settings → Your ClaxBot, where they set it; on the instance's,
 * check it under Settings → Model provider (an admin, with the link) or
 * ask an admin to. */
export function errorDetail(d: ErrorDetail, admin: boolean, keySource?: KeySource): ErrorWords {
  const cls = Object.hasOwn(LEADS, d.class) ? d.class : "api";
  const lead = cls === "api" && (d.type === "not_found_error" || d.status === 404)
    ? "The model provider does not know this model, or this key may not use it."
    : LEADS[cls];
  const msg = d.message?.trim() ?? "";
  const said = !msg ? null : OWN_WORDS.has(cls) ? `Details: ${msg}` : `The provider said: “${msg}”`;
  const meta = [d.status !== null && d.status !== undefined ? `HTTP ${d.status}` : "", d.type ?? "", d.request_id ? `request ${d.request_id}` : ""].filter(Boolean).join(" · ");
  const next = PASSING.has(cls) ? { text: "Try again in a little while." }
    : keySource === "own" ? { text: "Check your key under Settings → Your ClaxBot.", href: "/settings/claxbot#me-k" }
    : admin ? { text: "Check the key and the model under Settings → Model provider.", href: "/settings/provider" }
    : { text: "An admin can check the key and the model under Settings → Model provider." };
  return { lead, said, meta, next };
}

/** Clax's own message for a model call on the instance's key that failed,
 * as anyone but an admin sees it (`BotStep.instance_problem`): what
 * happened (`lead`), what it means for them (`body`), and the run to quote
 * to an admin (`ref`). Never anything the provider said. */
export type InstanceProblemWords = { lead: string; body: string; ref: string };
export function instanceProblemWords(p: { busy: boolean }, runId: string): InstanceProblemWords {
  const ref = `If you ask an admin about it, quote run ${runId}.`;
  return p.busy
    ? { lead: "This Clax's model provider is busy.", body: "Try again in a little while.", ref }
    : { lead: `${INSTANCE_MODEL_ERROR}.`, body: "This is a problem with how this Clax is set up, not with your request. It has been recorded for the people who run this Clax; try again later.", ref };
}

/** Whether a step's card is open: the live one, and any step that failed,
 * are; the others are collapsed until opened. */
export const openByDefault = (s: BotStep): boolean => s.status === "running";

function upsertStep(steps: BotStep[], step: BotStep): BotStep[] {
  const i = steps.findIndex(s => s.n === step.n);
  if (i < 0) return [...steps, step].sort((a, b) => a.n - b.n);
  const next = steps.slice();
  next[i] = { ...steps[i], ...step };
  return next;
}
function patchStep(steps: BotStep[], n: number, f: (s: BotStep) => Partial<BotStep>): BotStep[] {
  return steps.map(s => (s.n === n ? { ...s, ...f(s) } : s));
}

/** The run after one of its events. An event about a step not yet known
 * (a delta before its start) starts it. */
export function applyEvent(run: BotRun, ev: RunEvent): BotRun {
  const d = ev.data;
  const known = (n: number, kind: BotStep["kind"]) => run.steps.some(s => s.n === n) ? run.steps : upsertStep(run.steps, { n, kind, status: "running", started_at: new Date(0).toISOString(), credits_milli: 0 });
  switch (ev.event) {
    // A step begun again (a new attempt after a restart) starts over: its partial state is replaced, not merged.
    case "step_started": { const s = (d as { step: BotStep }).step; return { ...run, status: run.status === "queued" ? "running" : run.status, steps: [...run.steps.filter(x => x.n !== s.n), s].sort((a, b) => a.n - b.n) }; }
    case "turn_delta": {
      // `type` says which part of the turn the text extends: what it says,
      // its reasoning summary, or the input of the tool it calls.
      const x = d as Extract<RunEvent, { event: "turn_delta" }>["data"];
      const add = (v?: string) => (v ?? "") + x.text;
      return { ...run, steps: patchStep(known(x.step, "turn"), x.step, s => (x.type === "thinking" ? { thinking: add(s.thinking) } : x.type === "tool_input" ? { input: add(s.input) } : { text: add(s.text) })) };
    }
    case "script": { const x = d as { step: number; title: string; code: string }; return { ...run, steps: patchStep(known(x.step, "script"), x.step, () => ({ kind: "script", title: x.title, code: x.code })) }; }
    case "call": { const x = d as { step: number; call: BotCall }; return { ...run, steps: patchStep(known(x.step, "script"), x.step, s => ({ calls: [...(s.calls ?? []), x.call] })) }; }
    case "step_done": { const x = d as { step: BotStep; run?: Partial<BotRun> }; return { ...run, ...x.run, steps: upsertStep(run.steps, { ...x.step, status: "done" }) }; }
    case "run_end": { const x = d as { run: Partial<BotRun> }; return { ...run, ...x.run, steps: run.steps.map(s => (s.status === "running" ? { ...s, status: "done" } : s)) }; }
  }
  return run;
}

/** What a running turn is doing, in a few words, with how long it has run
 * ("Writing “Draw the map” · 42 lines · 1:05"): writing a tool call's
 * input once one streams (a script's title and line count as far as
 * written), else thinking. */
export function turnActivity(s: Pick<BotStep, "input" | "started_at">, now: number): string {
  const ms = now - Date.parse(s.started_at);
  const secs = Number.isFinite(ms) ? Math.max(0, Math.floor(ms / 1000)) : 0;
  const took = `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;
  if (!s.input) return `Thinking · ${took}`;
  const t = /"title"\s*:\s*"((?:[^"\\]|\\.)*)"/.exec(s.input)?.[1];
  let title = "";
  try { title = t ? JSON.parse(`"${t}"`) : ""; } catch { title = ""; }
  const code = /"code"\s*:\s*"/.exec(s.input);
  const lines = code ? s.input.slice(code.index).split("\\n").length : 0;
  return `Writing ${title ? `“${title}”` : "a script"}${lines ? ` · ${lines} line${lines === 1 ? "" : "s"}` : ""} · ${took}`;
}

/** The last `max` characters of `text`, from a word boundary, marked when cut. */
export function tail(text: string, max: number): string {
  if (text.length <= max) return text;
  const cut = text.slice(-max);
  return `…${cut.slice(Math.max(0, cut.indexOf(" ") + 1))}`;
}

/** A finished step, or the run's end, in one line (the run panel's live line). */
export function stepLine(ev: RunEvent, run: BotRun): string {
  if (ev.event === "run_end") return `ClaxBot: ${runStatus(run)}.`;
  if (ev.event !== "step_done") return "";
  const s = ev.data.step;
  const first = (t: string | undefined) => (t ?? "").split("\n")[0].slice(0, 140);
  switch (s.kind) {
    case "turn": return s.text ? `ClaxBot: ${first(s.text)}` : "";
    case "script": { const n = s.calls?.length ?? 0; return `Script “${s.title ?? "Script"}” ${s.error ? "failed" : "done"}, ${n} call${n === 1 ? "" : "s"}.`; }
    case "finish": return `Done: ${first(s.summary)}`;
    case "give_up": return `Gave up: ${first(s.reason)}`;
    default: {
      if (s.instance_problem) return s.instance_problem.busy ? "Stopped: this Clax's model provider is busy." : `Stopped: ${INSTANCE_MODEL_ERROR}.`;
      const w = stopWords(s.reason, run.key_source);
      const lead = s.error_detail ? ` ${errorDetail(s.error_detail, false).lead}` : "";
      return `${w ? `Stopped: ${w}.` : "Stopped."}${lead}`;
    }
  }
}

/** Parses one SSE message into a run event, or null for one this shell does not know. */
export function parseEvent(m: { event: string; data: string }): RunEvent | null {
  if (!["step_started", "turn_delta", "script", "call", "step_done", "run_end"].includes(m.event)) return null;
  try { return { event: m.event, data: JSON.parse(m.data) } as RunEvent; } catch { return null; }
}

/** What a run spent on tokens and on the rest, for its panel's line. */
export function runCost(r: Pick<BotRun, "credits_milli" | "tokens" | "key_source">): string {
  const t = r.tokens ? r.tokens.input + r.tokens.cached + r.tokens.output : 0;
  const tok = t ? ` · ${tokens(t)} tokens${r.key_source === "own" ? " on your key" : ""}` : "";
  return `This run: ${credits(r.credits_milli)} credits${tok}`;
}

// ---- audit ----

export type Range = "24h" | "7d" | "week" | "30d" | "all";
export const RANGES: { id: Range; label: string }[] = [
  { id: "24h", label: "Last 24 hours" }, { id: "7d", label: "7 days" }, { id: "week", label: "This week" }, { id: "30d", label: "30 days" }, { id: "all", label: "Any time" },
];
/** The start of range `r` before `now`, RFC 3339 (none for any time). The week starts Monday 00:00 UTC. */
export function rangeFrom(r: Range, now: Date): string | undefined {
  const day = 86_400_000;
  if (r === "24h") return new Date(now.getTime() - day).toISOString();
  if (r === "7d") return new Date(now.getTime() - 7 * day).toISOString();
  if (r === "30d") return new Date(now.getTime() - 30 * day).toISOString();
  if (r === "week") {
    const d = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate()));
    const back = (d.getUTCDay() + 6) % 7;
    return new Date(d.getTime() - back * day).toISOString();
  }
  return undefined;
}
export const GROUPS = [
  { id: "", label: "All kinds" }, { id: "views", label: "Views" }, { id: "changes", label: "Changes" },
  { id: "bot", label: "ClaxBot runs" }, { id: "credits", label: "Credits & limits" }, { id: "admin", label: "Admin" },
] as const;
/** Who acted, as the log says it: "ClaxBot for Sam", a person's name, else their ID. */
export function actorLabel(e: { actor: { id: string; name: string | null }; on_behalf_of: { id: string; name: string | null } | null }): string {
  const who = e.actor.name ?? (e.actor.id === "agent:claxbot" ? "ClaxBot" : e.actor.id);
  return e.on_behalf_of ? `${who} for ${e.on_behalf_of.name ?? e.on_behalf_of.id}` : who;
}

// ---- settings ----

/** The settings in sections by their first name part (`serve`, `bot`, …),
 * in the daemon's order, each field named by the rest of its name. */
export function sections(fields: SettingField[]): { name: string; fields: Setting[] }[] {
  const out = new Map<string, Setting[]>();
  for (const f of fields) {
    const dot = f.key.indexOf(".");
    const name = dot < 0 ? "general" : f.key.slice(0, dot);
    const list = out.get(name) ?? [];
    list.push({ ...f, label: dot < 0 ? f.key : f.key.slice(dot + 1) });
    out.set(name, list);
  }
  return [...out].map(([name, list]) => ({ name, fields: list }));
}
/** Whether a setting names a model (`bot.models.default`, `sample.models.quick`): its input is the model selector. */
export const isModel = (key: string): boolean => /\.models\.[^.]+$/.test(key);
/** A setting's value as its input holds it, and back. */
export const asText = (v: unknown): string => (v === null || v === undefined ? "" : typeof v === "string" ? v : typeof v === "object" ? JSON.stringify(v) : String(v));
export function fromText(text: string, was: unknown): unknown {
  if (typeof was === "number") { const n = Number(text); return text.trim() === "" || Number.isNaN(n) ? was : n; }
  if (typeof was === "object" && was !== null) { try { return JSON.parse(text); } catch { return was; } }
  if (was === null && text.trim() === "") return null;
  if (was === null && /^-?\d+$/.test(text.trim())) return Number(text);
  return text;
}

// ---- event IDs ----

/** A run event's ID (`<step>.<attempt>:<seq>`) as numbers, or null when it is not one. */
export function eventId(id: string | null | undefined): [number, number, number] | null {
  const m = /^(\d+)\.(\d+):(\d+)$/.exec(id ?? "");
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}
/** Whether event `id` comes after `last` (none yet: every one does), by
 * (step, attempt, seq). An ID not of that shape is taken as new. */
export function after(id: string | undefined, last: string | null): boolean {
  const a = eventId(id);
  const b = eventId(last);
  if (!a || !b) return true;
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i];
  return false;
}
