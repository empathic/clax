// ClaxBot's HTTP API (docs/superpowers/specs/2026-10-21-claxbot-design.md
// §10): the caller's own ClaxBot state and key, runs and their events, the
// admin pages (people and flags, provider, pricing, usage), the localhost
// daemon's settings, and the audit log. Every call is the browser's: the
// owner's shell sends its token (`getToken`), anyone else's request is
// their cookie's. A refusal throws `Refused` with the daemon's status,
// message and code, which the pages show as they are. Names, titles,
// summaries and bodies are people's and agents' text: untrusted, rendered
// as text. Keys are write-only: no response carries one, only its
// `KeyInfo`. Credits are integer milli-credits, money integer micro-USD,
// margins integer basis points (§7.1).
import { getToken } from "../api";
import { Refused } from "../held";

export { Refused };

/** Why ClaxBot is not available to the caller (§6.4). */
export type Unavailable = "no_flag" | "no_credits" | "not_configured" | "model_not_priced";
/** Where a value comes from (§8): its default, the database, `config.toml`, or an environment variable. */
export type Source = "default" | "db" | "file" | `env:${string}`;
/** A stored key, never the key itself (§8.1); `set_by` is who set it, `{id, name}` as every actor. */
/** A stored key's metadata (§8.1). `host` is the origin it was set for and
 * is only ever sent to; `problem`, when the provider in effect is another,
 * says so ("your key: it was set for <host>; set it again"). */
export type KeyInfo = { set: boolean; last4: string | null; set_at: string | null; set_by?: { id: string; name: string | null } | null; last_ok_at: string | null; host?: string | null; problem?: string | null };
/** One week of credits (§7.4): `start` and `end` are RFC 3339 (Monday 00:00 UTC to the next). */
export type Week = { start: string; end: string; weekly_milli: number; lift_milli: number; spent_milli: number; warn: boolean; exhausted: boolean };
/** Whose model key a run, a turn or a person's runs use: `instance` or `own`, never null. */
export type KeySource = "instance" | "own";
/** A person key: a connected person's public ID, or `*owner` for the owner
 * (crates/clax-store/src/roles.rs). Every `person` field is one, and the
 * routes take one in their path, URL-encoded (`/api/admin/people/%2Aowner/flags`). */
export type PersonKey = string;
/** A person's role (§5). */
export type Role = "owner" | "admin" | "member";

/** `GET /api/me/bot`. `role` is the caller's role, which the shell reads to
 * show Admin; a daemon that leaves it out is read as `member`, except in
 * the owner's shell (it holds the token), which is the owner's.
 * `estimate_milli` is the typical credits of one run for this person, the
 * New dialog's line: the 25th to 75th percentile of the credits of their
 * last 20 finished runs, else of the instance's recent finished runs, else
 * a fixed default range from the settings; `can_export` whether `GET /api/toolpath/export`
 * admits the caller (the audit page's Export Toolpath); `provider_host` the
 * origin (scheme, host, port) a key set now is bound to and only ever sent
 * to. */
export type MeBot = {
  enabled: boolean; unenforced: boolean; available: boolean; unavailable_reason?: Unavailable | null;
  key: { own: KeyInfo | null; use_own: boolean };
  week: Week;
  role?: Role;
  estimate_milli?: { low: number; high: number } | null;
  can_export?: boolean;
  provider_host?: string | null;
};

/** A line of usage: a cost line's ID, its label and unit, and the quantity and credits (§7.3). */
export type LineTotal = { line: string; label?: string; unit?: string; qty: number; credits_milli: number; cost_micro_usd?: number };
/** One run as `GET /api/me/usage` lists it, with its per-line totals. */
export type UsageRun = {
  id: string; artifact_id: string | null; artifact_title: string | null; started_at: string;
  status: RunStatus; scripts: number; key_source: "instance" | "own"; credits_milli: number;
  /** Model tokens over the run (input, cached and output together). */
  tokens: number;
  /** Script compute over the run, in CPU·GB-seconds (shown; the ledger counts CPU·MiB·ms). */
  cpu_gb_s: number;
  lines: LineTotal[];
};
/** `GET /api/me/usage?week=`. */
export type MeUsage = { week: { start: string; end: string }; runs: UsageRun[]; totals: LineTotal[] };

// ---- runs ----

export type RunStatus = "queued" | "running" | "stopping" | "finished" | "gave_up" | "stopped";
/** Why a run stopped (§4.2, §7.4). */
export type StopReason = "credits_exhausted" | "run_cap" | "max_steps" | "max_time" | "cancelled" | string;
/** One `clax` API call a script made: its op, what it named, how it went and how long it took. */
export type BotCall = { op: string; target?: string | null; ok: boolean; ms: number; error?: string | null };
export type StepKind = "turn" | "script" | "finish" | "give_up" | "stopped";
/** One step of a run (§4.2). A `turn` carries the model's narration
 * (`text`) and tokens; a `script` its title, code, calls, logs, return
 * value and error; `finish`, `give_up` and `stopped` their summary or reason. */
export type BotStep = {
  n: number; kind: StepKind; status: "running" | "done"; started_at: string; ended_at?: string | null;
  credits_milli: number;
  text?: string; model?: string; input_tokens?: number; cached_tokens?: number; output_tokens?: number; key_source?: "instance" | "own";
  title?: string; code?: string; calls?: BotCall[]; logs?: string[]; value?: unknown; error?: string | null; cpu_ms?: number;
  summary?: string; reason?: string;
  /** Live only, from a turn's `turn_delta`s: the reasoning summary as it
   * streams, and the tool call's input (JSON, partial) as it is written. */
  thinking?: string; input?: string;
};
/** `GET /api/bot/runs/<id>`: a run with its steps. `draft` is set while the
 * run holds an unpublished draft (§6.3), and `url` opens it.
 * `last_event_id` is the ID of the newest event this state includes (null
 * before the first): the panel opens the events with it as
 * `Last-Event-ID`, so it hears every later event once (`RunEvent`). */
export type BotRun = {
  id: string; artifact_id: string; thread_ids: string[]; request: "thread" | "brief";
  status: RunStatus; reason?: StopReason | null;
  model: string; key_source: "instance" | "own";
  /** The person the run acts for: their person key (`*owner` for the owner) and name. */
  acting_for?: { id: string; name: string | null } | null;
  started_at: string; ended_at?: string | null;
  credits_milli: number;
  tokens?: { input: number; cached: number; output: number };
  draft?: { url: string } | null;
  last_event_id?: string | null;
  steps: BotStep[];
};
/** A run as `GET /api/bot/runs?artifact=` lists it: without its steps. */
export type RunSummary = Omit<BotRun, "steps">;

/** The events of `GET /api/bot/runs/<id>/events` (§10), each event's `data`
 * as JSON. Every event carries an SSE `id` of `<step>.<attempt>:<seq>`, all
 * decimal: `attempt` counts the times the step was begun (a step begun but
 * not committed before a crash is begun again with the next attempt), so
 * no ID repeats within a run, and IDs order by (step, attempt, seq). A
 * request with `Last-Event-ID` gets every event after that ID and then the
 * live ones; without it, from the run's first event. After a restart the
 * replay is rebuilt from the committed steps (each as `step_started` and
 * `step_done` with its final content, no deltas) plus the live tail of the
 * open attempt. The client sends the last ID it applied (on its first
 * request, the run's `last_event_id`), drops any event at or below it, and
 * takes a `step_started` for a step it holds as that step begun again: it
 * replaces the step's partial state. */
export type RunEvent =
  | { event: "step_started"; data: { step: BotStep } }
  | { event: "turn_delta"; data: { step: number; type?: "text" | "thinking" | "tool_input"; text: string } }
  | { event: "script"; data: { step: number; title: string; code: string } }
  | { event: "call"; data: { step: number; call: BotCall } }
  | { event: "step_done"; data: { step: BotStep; run?: Partial<RunSummary> } }
  | { event: "run_end"; data: { run: Partial<RunSummary> } };

// ---- admin ----

/** A person as `GET /api/admin/people` lists them. `refused_week` counts
 * this week's `bot.refused` audit events for them (§9.1): a send or a new
 * artifact refused before any model call. */
export type AdminPerson = {
  person: PersonKey; name: string | null; role: Role;
  flags: { claxbot: boolean; claxbot_unenforced: boolean };
  /** The key their next run would use (their own while set and turned on, else the instance's). */
  key_source: KeySource;
  week: { weekly_milli: number; lift_milli: number; spent_milli: number };
  tokens_week: number;
  /** Their runs this week, and their runs refused at their limit (§7.4). */
  runs_week: number; refused_week: number;
};
export type Models = { default: string; quick: string; complex: string };
/** The wire API the provider at the base URL speaks. */
export type ModelApi = "anthropic" | "openai" | "gemini";
/** `GET /api/admin/provider`. `caps` are the run limits by config name
 * (`max_steps`, `max_run_credits`, …); `sources` say where each field's
 * value comes from, by its dotted name (`api`, `base_url`, `models.default`,
 * `caps.max_steps`, `key`); a field from the file or the environment is
 * read-only here. `kek` says whether a key-encryption key seals the keys
 * and which, by fingerprint, for checking a backup of it. */
export type Provider = {
  api: ModelApi; base_url: string; models: Models; key: KeyInfo;
  caps: Record<string, number>; sources: Record<string, Source>;
  kek?: { set: boolean; fingerprint: string | null };
};
/** What `PUT /api/admin/provider` changes: any of the editable fields. */
export type ProviderPatch = { api?: ModelApi; base_url?: string; models?: Partial<Models>; caps?: Record<string, number> };
/** A listed model's price now, as `GET /api/admin/provider/models` gives
 * it: the source its four token units share, else `mixed`; whether any
 * unit is an estimate (priced from the catalog or the default rates); and
 * the milli-credits a million input and output tokens charge (`source` and
 * the credits `null` when it resolves to no price). */
export type PriceSummary = { source: Exclude<RateSource, "provider"> | "mixed" | null; estimated: boolean; input_credits_milli_per_mtok: number | null; output_credits_milli_per_mtok: number | null };
export type ProviderModels = { models: { id: string; display_name: string; pricing?: PriceSummary | null }[]; fetched_at: string };
export type Unit = "mtok_input" | "mtok_cached_input" | "mtok_cache_write" | "mtok_output" | "cpu_gb_s" | "mreq" | "gb_month" | "run" | "kreq";
/** A cost line (§7.2). `fixed_credits_milli`: a line charged at fixed credits per
 * unit (the harness fee) rather than from its cost. */
export type CostLine = {
  id: string; label: string; unit: Unit; cost_micro_usd: number; margin_bp: number;
  applies: "instance_key" | "always"; source: string; model?: string | null; fixed_credits_milli?: number | null;
};
/** Where a model's token rate comes from (docs/contract.md "Pricing"): its
 * cost line, the cached price catalog (tokenulator.dev's), the instance's
 * default rates, or (a usage row only) the provider's bill. */
export type RateSource = "line" | "catalog" | "default" | "provider";
/** A token unit as a rate card names it. */
export type TokenUnit = "input" | "cached_input" | "cache_write" | "output";
/** One unit of a model as it resolves now: every rate field `null`, with
 * `source`, when it resolves to none; `line_source` the cost line's own
 * source text (`null` without a line). `credits_milli_per_mtok` is what a
 * million tokens charge (rounded up); `catalog_micro_usd` the catalog's
 * cost for the unit; `drift` whether a cost-based line's cost differs from
 * it by more than 1 %. */
export type UnitPricing = {
  source: RateSource | null; line_source: string | null; cost_micro_usd: number | null; margin_bp: number | null; fixed_credits_milli: number | null;
  applies: "instance_key" | "always" | null; credits_milli_per_mtok: number | null; catalog_micro_usd: number | null; drift: boolean;
};
/** The price catalog's listing of a model, in micro-USD per million tokens (cache rates `null` where it lists none). */
export type CatalogListing = {
  provider_id: string; model_id: string; name: string; input_micro_usd: number; output_micro_usd: number;
  cache_read_micro_usd: number | null; cache_write_micro_usd: number | null; source: string; observed_at: string;
};
/** A model the pricing page shows: its display name (`null` when it has
 * none), the tiers it is in effect for, each unit's rate, the catalog's
 * listing and whether any unit drifted. */
export type ModelPricing = { model: string; name: string | null; tiers: (keyof Models)[]; units: Record<TokenUnit, UnitPricing>; listing: CatalogListing | null; drift: boolean };
/** `GET /api/admin/pricing`. `default_rates` are milli-credits per million
 * tokens; `catalog` the cache's age and the catalog provider in effect
 * (`null` for a host it does not name); `lines` every cost line but the
 * token lines, which belong to `models`' rate cards; `token_margin_bp` the margin
 * tokenulator.dev's prices are charged at; `sources` where the credit price,
 * the free tier, each `default_rates.<unit>` and `require_explicit_price`
 * come from. */
export type Pricing = {
  credit_price_micro_usd: number; free_weekly_milli: number; lines: CostLine[];
  default_rates: Record<TokenUnit, number>; require_explicit_price: boolean; token_margin_bp: number;
  models: ModelPricing[]; catalog: { fetched_at: string | null; provider: string | null }; sources: Record<string, Source>;
};
/** What `PUT /api/admin/pricing` takes: the lines as `GET` lists them (one
 * left out is deleted; a token line is refused, and changes only as a rate
 * card); the default rates and `require_explicit_price` may be left out. */
export type PricingPatch = Pick<Pricing, "credit_price_micro_usd" | "free_weekly_milli" | "lines"> & { default_rates?: Partial<Record<TokenUnit, number>>; require_explicit_price?: boolean };
/** One unit of a rate card: cost-based (`fixed_credits_milli` `null`: cost per million tokens plus margin) or credit-based (fixed milli-credits per million tokens, the cost for information). */
export type UnitRate = { cost_micro_usd: number; margin_bp: number; fixed_credits_milli: number | null };
/** `POST /api/admin/pricing/models`: a model's rate card; `name` labels its lines (left out: the name it has, else the model ID). */
export type ModelRates = { model: string; name?: string } & Record<TokenUnit, UnitRate>;
/** `GET /api/admin/usage?week=`. `cost_micro_usd` is what the paid credits
 * cost; `free_cost_micro_usd` what the free tier's did. */
export type AdminUsage = {
  week: { start: string; end: string };
  paid_milli: number; free_milli: number;
  /** Runs this week, and sends and new artifacts refused (the week's `bot.refused` events). */
  runs: number; refused: number;
  revenue_micro_usd: number; cost_micro_usd: number; free_cost_micro_usd: number;
  by_person: { person: string; name: string | null; credits_milli: number; free_milli: number; cost_micro_usd: number; runs: number; tokens: number }[];
  by_line: (LineTotal & { label: string; unit: Unit; cost_micro_usd: number })[];
};

// ---- settings (the localhost daemon) ----

/** One config field (§8; crates/clax-server/src/routes/settings.rs): its
 * dotted name, the value in effect (as the daemon read it when it started)
 * and where that came from; `pending`, the value it takes at its next start
 * when that differs; whether this screen may change it (not when the
 * environment sets it, nor `bin`); whether a change waits for a restart;
 * the variable that would set it; and a note (`serve.port`: what chose the
 * port it listens on). No field holds a secret: `*_env` fields name a
 * variable. */
export type SettingField = {
  key: string; value: unknown; source: Source; pending?: unknown; editable: boolean; restart_required: boolean; env: string | null; note?: string;
};
/** `GET /api/settings`: the file, every field, the tables whose values do
 * not check (the daemon runs without them), and, when the file did not
 * read at the start (the daemon runs on the defaults), why. `PUT
 * /api/settings {set: {key: value | null}}` answers the same with the keys
 * `written` and whether they wait for a restart. */
export type Settings = {
  file: string; fields: SettingField[]; problems: { table: string; message: string }[];
  started_with_invalid_config?: boolean; start_error?: string | null;
  written?: string[]; restart_required?: boolean;
};

// ---- audit ----

/** Who acted, as the audit route resolves it: a person key (`p_…`, or
 * `*owner` named as the owner), `agent:claxbot` (ClaxBot), another agent's
 * handle, or `*system` (named "Clax"), each with its name. `on_behalf_of`
 * and a key's `set_by` are resolved the same way. */
export type AuditActor = { id: string; name: string | null };
export type AuditEvent = {
  seq: number; at: string; actor: AuditActor; on_behalf_of: AuditActor | null;
  kind: string; summary: string; artifact_id: string | null; run: string | null;
};
/** `GET /api/audit/<seq>`: the event with its stored body. */
export type AuditDetail = AuditEvent & { body: unknown };
export type AuditGroup = "views" | "changes" | "bot" | "credits" | "admin";
/** `actor`: `claxbot` (ClaxBot acting for anyone, the store's `agent:claxbot`) or a person key (that person and their agents, ClaxBot left out). */
export type AuditFilter = { q?: string; actor?: string; group?: AuditGroup; artifact?: string; from?: string; to?: string; cursor?: number };
/** `next_cursor`: the `seq` the next page starts below, when there is one. */
export type AuditPage = { events: AuditEvent[]; next_cursor: number | null };

// ---- calls ----

async function headers(json: boolean): Promise<Record<string, string>> {
  const h: Record<string, string> = json ? { "content-type": "application/json" } : {};
  const t = await getToken();
  if (t) h.authorization = `Bearer ${t}`;
  return h;
}

async function call<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const init: RequestInit = { method, headers: await headers(body !== undefined) };
  if (method === "GET") init.cache = "no-store";
  if (body !== undefined) init.body = JSON.stringify(body);
  const res = await fetch(path, init);
  if (res.ok) return (res.status === 204 ? {} : await res.json()) as T;
  let b: { error?: { code?: string; message?: string; run?: { id?: string } } } = {};
  try { b = await res.json(); } catch { /* not json */ }
  if (res.status === 409 && b.error?.code === "run_active" && b.error.run?.id) throw new RunActive(b.error.run.id);
  throw new Refused(res.status, b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
}
const e = encodeURIComponent;
const query = (q: Record<string, string | number | undefined>) => {
  const s = Object.entries(q).filter(([, v]) => v !== undefined && v !== "").map(([k, v]) => `${k}=${e(String(v))}`).join("&");
  return s ? `?${s}` : "";
};

/** The caller's ClaxBot, or null where the daemon has none (404). */
export async function meBot(): Promise<MeBot | null> {
  try { return await call<MeBot>("/api/me/bot"); } catch (x) { if (x instanceof Refused && x.status === 404) return null; throw x; }
}
export const setOwnKey = (key: string) => call<MeBot>("/api/me/bot/key", "PUT", { key });
export const clearOwnKey = () => call<MeBot>("/api/me/bot/key", "DELETE");
export const useOwnKey = (use_own: boolean) => call<MeBot>("/api/me/bot", "PUT", { use_own });
export const meUsage = (week?: string) => call<MeUsage>(`/api/me/usage${query({ week })}`);

/** A send refused because run `run` already works on one of its threads (409 `run_active`). */
export class RunActive extends Error {
  constructor(public run: string) { super("ClaxBot is already working on this thread"); this.name = "RunActive"; }
}

/** Starts a run of `thread_ids`: the run made, or (`active`) the run that
 * already works on one of them, which a second send never duplicates. */
export async function startRun(artifact_id: string, thread_ids: string[]): Promise<{ run: { id: string }; active: boolean }> {
  try {
    const { run } = await call<{ run: RunSummary }>("/api/bot/runs", "POST", { thread_ids, artifact_id });
    return { run, active: false };
  } catch (x) {
    if (x instanceof RunActive) return { run: { id: x.run }, active: true };
    throw x;
  }
}
export const getRun = (id: string) => call<BotRun>(`/api/bot/runs/${e(id)}`);
export const listRuns = (artifact: string) => call<{ runs: RunSummary[] }>(`/api/bot/runs${query({ artifact })}`);
export const cancelRun = (id: string) => call<{ run: RunSummary }>(`/api/bot/runs/${e(id)}/cancel`, "POST", {});
/** The run's event stream after event `after` (`Last-Event-ID`; none: from
 * its first), as the response whose body `readSse` reads. */
export async function runEvents(id: string, signal: AbortSignal, after?: string | null): Promise<Response> {
  const h: Record<string, string> = { ...await headers(false), accept: "text/event-stream" };
  if (after) h["last-event-id"] = after;
  return fetch(`/api/bot/runs/${e(id)}/events`, { headers: h, cache: "no-store", signal });
}

export const adminPeople = () => call<AdminPerson[]>("/api/admin/people");
export const setFlags = (person: string, flags: Partial<AdminPerson["flags"]>) => call<AdminPerson>(`/api/admin/people/${e(person)}/flags`, "PUT", flags);
export const setRole = (person: string, role: "admin" | "member") => call<AdminPerson>(`/api/admin/people/${e(person)}/role`, "PUT", { role });
export const setLift = (person: string, lift_milli: number, reason: string) => call<AdminPerson>(`/api/admin/people/${e(person)}/credits`, "PUT", { lift_milli, reason });
export const provider = () => call<Provider>("/api/admin/provider");
export const setProvider = (patch: ProviderPatch) => call<Provider>("/api/admin/provider", "PUT", patch);
export const setInstanceKey = (key: string) => call<Provider>("/api/admin/provider/key", "PUT", { key });
export const clearInstanceKey = () => call<Provider>("/api/admin/provider/key", "DELETE");
export const testProvider = () => call<{ ok: boolean; latency_ms: number; error?: string | null }>("/api/admin/provider/test", "POST", {});
export const providerModels = () => call<ProviderModels>("/api/admin/provider/models");
/** `POST /api/admin/pricing/tokenulator`: refreshes the price catalog, then
 * prices `model`'s token lines from it, as `providerId` (its tokenulator.dev
 * provider ID) lists it, or as the provider the API and base URL name. */
export const priceFromTokenulator = (model: string, providerId?: string) => call<Pricing>("/api/admin/pricing/tokenulator", "POST", providerId ? { model, provider: providerId } : { model });
export const pricing = () => call<Pricing>("/api/admin/pricing");
export const setPricing = (p: PricingPatch) => call<Pricing>("/api/admin/pricing", "PUT", p);
/** Sets a model's rate card, its four token lines. */
export const setModelRates = (r: ModelRates) => call<Pricing>("/api/admin/pricing/models", "POST", r);
/** Deletes a model's token lines, so its tokens resolve past them (409 `cost_line_in_use` while a run on it has not ended). */
export const deleteModelRates = (model: string) => call<Pricing>(`/api/admin/pricing/models${query({ model })}`, "DELETE");
/** Fetches the price catalog now (502 `tokenulator_error` when it cannot be read; the cache is kept). */
export const refreshCatalog = () => call<Pricing>("/api/admin/pricing/catalog/refresh", "POST", {});
export const adminUsage = (week?: string) => call<AdminUsage>(`/api/admin/usage${query({ week })}`);

/** The daemon's settings, or null on a daemon that has none to show (404: a hosted one). */
export async function settings(): Promise<Settings | null> {
  try { return await call<Settings>("/api/settings"); } catch (x) { if (x instanceof Refused && x.status === 404) return null; throw x; }
}
/** Writes `set` (dotted name → value; null removes it) to `config.toml`. */
export const saveSettings = (set: Record<string, unknown>) => call<Settings>("/api/settings", "PUT", { set });

export const audit = (f: AuditFilter) => call<AuditPage>(`/api/audit${query(f)}`);
export const auditEvent = (seq: number) => call<AuditDetail>(`/api/audit/${seq}`);
/** The Toolpath export of `f`'s artifact and time range (docs/contract.md
 * "Export"), fetched as this browser (it is the owner's route) for a download. */
export async function exportToolpath(f: Pick<AuditFilter, "artifact" | "from" | "to">): Promise<Blob> {
  const res = await fetch(`/api/toolpath/export${query({ artifact: f.artifact, since: f.from, until: f.to })}`, { headers: await headers(false) });
  if (!res.ok) {
    let b: { error?: { code?: string; message?: string } } = {};
    try { b = await res.json(); } catch { /* not json */ }
    throw new Refused(res.status, b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
  }
  return res.blob();
}
