// ClaxBot's numbers and words for the shell (spec 2026-10-21-claxbot §7,
// §11): credits from milli-credits, money from micro-USD, margins from
// basis points; the week's meter and its warn and exhausted states; the
// Send menu's availability line; a cost line's credits per unit; the
// margin before and after the free tier; a run's state as its events
// arrive; the audit's time ranges; and the settings' sections. Pure: no
// clock but the one passed in.
import type { AdminUsage, BotCall, BotRun, BotStep, CostLine, MeBot, RunEvent, RunStatus, SettingField, Source, Unavailable, Unit, Week } from "./api";

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

/** The usage banner a page shows for `me`: none, a warning from 80 %, or
 * the exhausted state (not for an unenforced person, whom ClaxBot never stops). */
export type Banner = { kind: "warn" | "exhausted"; text: string; detail: string } | null;
export function banner(me: MeBot | null): Banner {
  if (!me?.enabled) return null;
  const m = meter(me.week);
  const reset = resetDay(me.week.end);
  if (m.exhausted && !me.unenforced) {
    return { kind: "exhausted", text: "This week's credits are used up.", detail: `You've used ${credits(m.spent)} of ${credits(m.total)} credits. Your pages, comments and local agents keep working; ClaxBot is paused until ${reset}.` };
  }
  if (m.warn || (m.exhausted && me.unenforced)) {
    return { kind: "warn", text: `You've used ${credits(m.spent)} of your ${credits(m.total)} credits this week.`, detail: me.unenforced ? `Your limit is not enforced: ClaxBot keeps working. Resets ${reset}.` : `ClaxBot keeps working until you reach it. Resets ${reset}.` };
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

/** The Send menu's line under ClaxBot: where it runs, whose key and the
 * credits left, or why it is unavailable. */
export function availability(live: boolean, reason: Unavailable | null | undefined, me: MeBot | null): string {
  if (!live) return unavailableText(reason ?? (me?.unavailable_reason ?? null), me?.week);
  if (!me) return "Runs in Clax";
  const key = me.key.use_own && me.key.own?.set ? "your key" : "instance key";
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

export const UNITS: Record<Unit, string> = {
  mtok_input: "1M input tokens", mtok_cached_input: "1M cached tokens", mtok_cache_write: "1M cache-write tokens", mtok_output: "1M output tokens",
  cpu_gb_s: "CPU·GB-s", mreq: "1M requests", gb_month: "GB-month", run: "run",
};

/** The base unit a usage row counts in (§7.3), by its line's unit. */
export const BASE_UNITS: Record<Unit, string> = {
  mtok_input: "tokens", mtok_cached_input: "tokens", mtok_cache_write: "tokens", mtok_output: "tokens",
  cpu_gb_s: "CPU·MiB·ms", mreq: "requests", gb_month: "byte·s", run: "runs",
};

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
};
/** The run's state in a few words. */
export function runStatus(r: Pick<BotRun, "status" | "reason">): string {
  switch (r.status) {
    case "queued": return "Starting…";
    case "running": return "Working";
    case "stopping": return "Stopping…";
    case "finished": return "Finished";
    case "gave_up": return "Gave up";
    case "stopped": return r.reason ? `Stopped: ${REASONS[r.reason] ?? r.reason}` : "Stopped";
  }
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
    case "turn_delta": { const x = d as { step: number; text: string }; return { ...run, steps: patchStep(known(x.step, "turn"), x.step, s => ({ text: (s.text ?? "") + x.text })) }; }
    case "script": { const x = d as { step: number; title: string; code: string }; return { ...run, steps: patchStep(known(x.step, "script"), x.step, () => ({ kind: "script", title: x.title, code: x.code })) }; }
    case "call": { const x = d as { step: number; call: BotCall }; return { ...run, steps: patchStep(known(x.step, "script"), x.step, s => ({ calls: [...(s.calls ?? []), x.call] })) }; }
    case "step_done": { const x = d as { step: BotStep; run?: Partial<BotRun> }; return { ...run, ...x.run, steps: upsertStep(run.steps, { ...x.step, status: "done" }) }; }
    case "run_end": { const x = d as { run: Partial<BotRun> }; return { ...run, ...x.run, steps: run.steps.map(s => (s.status === "running" ? { ...s, status: "done" } : s)) }; }
  }
  return run;
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
    default: return `Stopped: ${first(s.reason)}`;
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
