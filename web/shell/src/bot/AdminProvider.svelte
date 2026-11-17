<svelte:options css="injected" />

<script lang="ts">
  // Settings, Model provider (spec 2026-10-21-claxbot D3, D15, §8.1): the model API
  // ClaxBot calls with the instance key: which API it speaks (Anthropic's
  // Messages, OpenAI's Chat Completions, Gemini; picking one fills in a
  // known base URL for it unless one of its own is set), its base URL, the key
  // (write-only: replace or clear it, never read it), the default, quick and
  // complex models chosen from what the provider lists for that key and URL
  // (↻ lists again; a custom ID for one it does not list; disabled until the
  // list loads), and the run caps. Each field shows where its value comes
  // from; one set in `config.toml` or the environment is read-only here.
  // Save sends the fields changed; Test connection makes one call with the
  // effective key and says how it went, and how the call failed when it
  // did. Under the key, its last failed request, until one goes through.
  // Failures: the instance key's failed requests, newest first (loaded on
  // mount, after Test connection and after the key changes), each with
  // when, whose, the run or the admin action, the model, how it failed in
  // words, the provider's status, type, request ID and words (text only),
  // and whether it came before the key was last set. A person's name
  // narrows the list to them until Show everyone; Show more pages on.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { relativeTime } from "../format";
  import { type ErrorDetail, type ModelApi, type Models, type Provider, type ProviderFailure, type ProviderModels, ModelRefused, clearInstanceKey, provider, providerFailures, providerModels, setInstanceKey, setProvider, testProvider } from "./api";
  import { readOnlySource } from "./model";
  import KeyField from "./KeyField.svelte";
  import ModelError from "./ModelError.svelte";
  import ModelSelect from "./ModelSelect.svelte";
  import SourceBadge from "./SourceBadge.svelte";

  let { now }: { now?: () => Date } = $props();
  let p = $state<Provider | null>(null);
  let err = $state<string | null>(null);
  let saved = $state("");
  let models = $state<ProviderModels | null>(null);
  let why = $state<string | null>("Listing the provider's models…");
  let whyDetail = $state<ErrorDetail | null>(null);
  let base = $state("");
  let api = $state<ModelApi>("anthropic");
  let picked = $state<Models>({ default: "", quick: "", complex: "" });
  let caps = $state<Record<string, string>>({});
  let busy = $state(false);
  let test = $state<{ ok: boolean; text: string; detail?: ErrorDetail | null } | null>(null);
  let failures = $state<ProviderFailure[] | null>(null);
  let failNext = $state<string | null>(null);
  let failErr = $state<string | null>(null);
  let failWho = $state<{ id: string; name: string | null } | null>(null);
  let failMore = $state(false);
  // Each load's turn: a later load (a narrowing, a refresh) wins over an earlier one still answering.
  let failTurn = 0;
  async function loadFailures(more = false) {
    const turn = ++failTurn;
    failErr = null; failMore = more;
    try {
      const r = await providerFailures({ person: failWho?.id, before: more ? failNext ?? undefined : undefined });
      if (turn !== failTurn) return;
      failures = more ? [...(failures ?? []), ...r.failures] : r.failures;
      failNext = r.next;
    } catch (e) { if (turn === failTurn) failErr = refusal(e); } finally { if (turn === failTurn) failMore = false; }
  }
  const narrow = (who: { id: string; name: string | null } | null) => { failWho = who; failures = null; failNext = null; void loadFailures(); };
  const ACTIONS: Record<string, string> = { "provider.test": "Test connection", "provider.models": "Model list" };
  const absolute = (iso: string) => new Date(iso).toLocaleString("en-US", { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
  const CAPS: Record<string, string> = { max_steps: "Max steps per run", max_run_credits: "Max credits per run", max_run_minutes: "Max minutes per run", max_tokens: "Max tokens per turn", concurrency: "Runs at once" };
  const MODELS: { id: keyof Models; label: string }[] = [{ id: "default", label: "Default" }, { id: "quick", label: "Quick (titles, summaries)" }, { id: "complex", label: "Complex (opt-in)" }];
  const ro = (field: string) => readOnlySource(p?.sources[field]);
  // Each API's known base URLs, the first its default.
  const APIS: { id: ModelApi; label: string; urls: string[] }[] = [
    { id: "anthropic", label: "Anthropic Messages API", urls: ["https://api.anthropic.com", "https://api.subconscious.dev"] },
    { id: "openai", label: "OpenAI Chat Completions (Mistral, Subconscious, …)", urls: ["https://api.mistral.ai/v1", "https://api.subconscious.dev/v1", "https://api.openai.com/v1"] },
    { id: "gemini", label: "Google Gemini API", urls: ["https://generativelanguage.googleapis.com/v1beta"] },
  ];
  const urls = $derived(APIS.find(a => a.id === api)?.urls ?? []);
  function pickApi(next: ModelApi) {
    api = next;
    const own = APIS.find(a => a.id === next)!.urls;
    // A base URL known for another API (or none) gives way to this one's default.
    if (!ro("base_url") && !own.includes(base.trim())) base = own[0];
  }

  function take(x: Provider) {
    p = x; base = x.base_url; api = x.api ?? "anthropic"; picked = { ...x.models };
    caps = Object.fromEntries(Object.entries(x.caps).map(([k, v]) => [k, String(v)]));
  }
  const load = () => provider().then(take, e => { err = refusal(e); });
  function listModels() {
    why = "Listing the provider's models…"; whyDetail = null;
    void providerModels().then(m => { models = m; why = null; }, e => { models = null; whyDetail = e instanceof ModelRefused ? e.detail : null; why = `The models could not be listed: ${refusal(e)} Set the key and the base URL, then list them again.`; });
  }
  async function save() {
    if (!p || busy) return;
    const patch: Parameters<typeof setProvider>[0] = {};
    if (!ro("api") && api !== p.api) patch.api = api;
    if (!ro("base_url") && base.trim() !== p.base_url) patch.base_url = base.trim();
    const m = Object.fromEntries(MODELS.filter(x => !ro(`models.${x.id}`) && picked[x.id] !== p!.models[x.id]).map(x => [x.id, picked[x.id]]));
    if (Object.keys(m).length) patch.models = m;
    const c: Record<string, number> = {};
    for (const [k, v] of Object.entries(caps)) {
      if (ro(`caps.${k}`) || Number(v) === p.caps[k]) continue;
      const n = Number(v);
      if (!Number.isInteger(n) || n < 0) { err = `${CAPS[k] ?? k} is a whole number, 0 or more.`; return; }
      c[k] = n;
    }
    if (Object.keys(c).length) patch.caps = c;
    if (!Object.keys(patch).length) { saved = "Nothing changed."; return; }
    busy = true; err = null; saved = "";
    const keyed = p.key.set && !p.sources.key?.startsWith("env:");
    try { take(await setProvider(patch)); saved = patch.base_url && keyed ? "Saved. Set the key again for the new base URL." : "Saved."; if (patch.base_url || patch.api) listModels(); } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  async function runTest() {
    test = { ok: true, text: "Testing…" };
    try {
      const r = await testProvider();
      test = r.ok ? { ok: true, text: `Connected in ${r.latency_ms} ms.` } : { ok: false, text: `The call failed${r.error ? `: ${r.error}` : "."}`, detail: r.detail ?? null };
    } catch (e) { test = { ok: false, text: refusal(e) }; }
    void loadFailures();
  }
  const keyChanged = async (fn: () => Promise<Provider>) => { take(await fn()); listModels(); void loadFailures(); };
  onMount(() => { void load(); listModels(); void loadFailures(); });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if p}
  <section class="bcard" aria-labelledby="pv-ep">
    <h3 id="pv-ep">Endpoint</h3>
    <div class="fields">
      <div class="field">
        <label for="pv-api">API <SourceBadge source={p.sources.api} /></label>
        <select id="pv-api" value={api} disabled={ro("api")} onchange={e => pickApi((e.currentTarget as HTMLSelectElement).value as ModelApi)}>
          {#each APIS as a (a.id)}<option value={a.id}>{a.label}</option>{/each}
        </select>
      </div>
      <div class="field">
        <label for="pv-base">Base URL <SourceBadge source={p.sources.base_url} /></label>
        <input id="pv-base" type="url" spellcheck="false" list="pv-base-urls" bind:value={base} readonly={ro("base_url")} />
        <datalist id="pv-base-urls">{#each urls as u (u)}<option value={u}></option>{/each}</datalist>
      </div>
      <div class="field">
        <span class="lbl">API key <SourceBadge source={p.sources.key} /></span>
        <KeyField info={p.key} label="Instance API key" readOnly={ro("key")} {now} onSave={k => keyChanged(() => setInstanceKey(k))} onClear={() => keyChanged(clearInstanceKey)} />
        {#if p.key.last_problem}<div class="pv-problem"><ModelError detail={p.key.last_problem} admin before={`The last request with this key failed ${relativeTime(p.key.last_problem.at, now?.() ?? new Date())}:`} next={false} /></div>{/if}
      </div>
    </div>
    {#if p.kek}<p class="note">{#if p.kek.set}Stored keys are sealed under the key-encryption key with fingerprint <code>{p.kek.fingerprint}</code>.{:else}No key-encryption key is set, so no API key can be stored.{/if}</p>{/if}
  </section>
  <section class="bcard" aria-labelledby="pv-m">
    <h3 id="pv-m">Models</h3>
    <div class="fields">
      {#each MODELS as m (m.id)}
        <div class="field">
          <ModelSelect id={`pv-model-${m.id}`} label={m.label} value={picked[m.id]} models={models?.models ?? null} why={m.id === "default" ? why : null} readOnly={ro(`models.${m.id}`)} onRefresh={listModels} onChange={v => { picked = { ...picked, [m.id]: v }; }} />
          <SourceBadge source={p.sources[`models.${m.id}`]} />
        </div>
      {/each}
    </div>
    {#if models}<p class="note">{models.models.length} models listed by the provider with this key.</p>{:else if whyDetail}<div class="pv-problem"><ModelError detail={whyDetail} admin next={false} /></div>{/if}
  </section>
  <section class="bcard" aria-labelledby="pv-c">
    <h3 id="pv-c">Caps</h3>
    <div class="fields">
      {#each Object.keys(caps) as k (k)}
        <div class="field">
          <label for={`pv-cap-${k}`}>{CAPS[k] ?? k} <SourceBadge source={p.sources[`caps.${k}`]} /></label>
          <input id={`pv-cap-${k}`} type="number" min="0" step="1" inputmode="numeric" bind:value={caps[k]} readonly={ro(`caps.${k}`)} />
        </div>
      {/each}
    </div>
  </section>
  <div class="acts">
    <button type="button" class="primary" aria-disabled={busy} onclick={save}>{busy ? "Saving…" : "Save"}</button>
    <button type="button" onclick={runTest}>Test connection</button>
    <span role="status">{#if test}<span class={test.ok ? "ok" : "err"}>{test.text}</span>{:else if saved}<span class="ok">{saved}</span>{/if}</span>
  </div>
  {#if test?.detail}<div class="pv-problem pv-test"><ModelError detail={test.detail} admin next={false} /></div>{/if}
  <section class="bcard" aria-labelledby="pv-f">
    <h3 id="pv-f">Failures</h3>
    <p class="note">Requests with the instance key that failed, newest first. Clax keeps the last 100 per person and 1,000 in all, for 30 days.</p>
    {#if failWho}<p class="pv-fwho">Showing only {failWho.name ?? failWho.id}'s. <button type="button" class="ghost" onclick={() => narrow(null)}>Show everyone</button></p>{/if}
    {#if failErr}<p class="note pv-ferr">The failures could not be listed: {failErr}</p>{/if}
    {#if failures?.length}
      <ol class="pv-fails" aria-label="Failed requests">
        {#each failures as f (f.id)}
          <li>
            <p class="pv-fhead">
              <time datetime={f.at} title={absolute(f.at)}>{relativeTime(f.at, now?.() ?? new Date())}</time>
              {#if f.person}<button type="button" class="ghost pv-fperson" title={`Only ${f.person.name ?? f.person.id}'s`} onclick={() => narrow(f.person)}>{f.person.name ?? f.person.id}</button>{/if}
              {#if ACTIONS[f.request]}<span>{ACTIONS[f.request]}</span>{:else if f.run_id}<a href={`/settings/audit?q=${encodeURIComponent(f.run_id)}`}>run <code>{f.run_id}</code></a>{/if}
              {#if f.model}<code>{f.model}</code>{/if}
              {#if !f.since_key_change}<span class="pv-fold">before the key was last set</span>{/if}
            </p>
            <ModelError detail={f} admin next={false} />
          </li>
        {/each}
      </ol>
      {#if failNext}<button type="button" disabled={failMore} onclick={() => loadFailures(true)}>{failMore ? "Loading…" : "Show more"}</button>{/if}
    {:else if failures}
      <p class="note">No failed requests with the instance key.</p>
    {:else if !failErr}
      <p class="note" role="status">Loading the failures…</p>
    {/if}
  </section>
{:else if !err}
  <p class="note" role="status">Loading the provider…</p>
{/if}

<style>
  :global {
    .pv-problem { font-size: 13px; padding: 8px 10px; border-radius: var(--radius-sm); border: 1px solid var(--border); background: var(--card); min-width: 0; }
    .pv-problem .merr-lead { color: var(--danger); }
    .pv-fails { list-style: none; margin: 0 0 10px; padding: 0; display: grid; gap: 8px; min-width: 0; }
    .pv-fails li { font-size: 13px; padding: 8px 10px; border-radius: var(--radius-sm); border: 1px solid var(--border); background: var(--card); min-width: 0; overflow-wrap: anywhere; }
    .pv-fhead { margin: 0; display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 10px; font-size: 12.5px; color: var(--muted); min-width: 0; }
    .pv-fhead code { font: 12px var(--mono); overflow-wrap: anywhere; min-width: 0; }
    .pv-fhead a { color: var(--accent-ink); text-decoration: underline; }
    .pv-fhead time { font-variant-numeric: tabular-nums; }
    .pv-fhead .pv-fperson { min-height: 0; padding: 0 2px; font-weight: 600; color: var(--fg); text-decoration: underline; text-underline-offset: 2px; white-space: normal; text-align: start; overflow-wrap: anywhere; }
    .pv-fold { font-style: italic; }
    .pv-fwho { margin: 0 0 8px; display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 8px; }
  }
</style>
