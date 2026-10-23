<svelte:options css="injected" />

<script lang="ts">
  // Admin › Provider (spec 2026-10-21-claxbot D3, D15, §8.1): the Anthropic
  // Messages API ClaxBot calls with the instance key: its base URL, the key
  // (write-only: replace or clear it, never read it), the default, quick and
  // complex models chosen from what the provider lists for that key and URL
  // (↻ lists again; a custom ID for one it does not list; disabled until the
  // list loads), and the run caps. Each field shows where its value comes
  // from; one set in `config.toml` or the environment is read-only here.
  // Save sends the fields changed; Test connection makes one call with the
  // effective key and says how it went.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type Models, type Provider, type ProviderModels, clearInstanceKey, provider, providerModels, setInstanceKey, setProvider, testProvider } from "./api";
  import { readOnlySource } from "./model";
  import KeyField from "./KeyField.svelte";
  import ModelSelect from "./ModelSelect.svelte";
  import SourceBadge from "./SourceBadge.svelte";

  let { now }: { now?: () => Date } = $props();
  let p = $state<Provider | null>(null);
  let err = $state<string | null>(null);
  let saved = $state("");
  let models = $state<ProviderModels | null>(null);
  let why = $state<string | null>("Listing the provider's models…");
  let base = $state("");
  let picked = $state<Models>({ default: "", quick: "", complex: "" });
  let caps = $state<Record<string, string>>({});
  let busy = $state(false);
  let test = $state<{ ok: boolean; text: string } | null>(null);
  const CAPS: Record<string, string> = { max_steps: "Max steps per run", max_run_credits: "Max credits per run", max_run_minutes: "Max minutes per run", max_tokens: "Max tokens per turn", concurrency: "Runs at once" };
  const MODELS: { id: keyof Models; label: string }[] = [{ id: "default", label: "Default" }, { id: "quick", label: "Quick (titles, summaries)" }, { id: "complex", label: "Complex (opt-in)" }];
  const ro = (field: string) => readOnlySource(p?.sources[field]);

  function take(x: Provider) {
    p = x; base = x.base_url; picked = { ...x.models };
    caps = Object.fromEntries(Object.entries(x.caps).map(([k, v]) => [k, String(v)]));
  }
  const load = () => provider().then(take, e => { err = refusal(e); });
  function listModels() {
    why = "Listing the provider's models…";
    void providerModels().then(m => { models = m; why = null; }, e => { models = null; why = `The models could not be listed: ${refusal(e)} Set the key and the base URL, then list them again.`; });
  }
  async function save() {
    if (!p || busy) return;
    const patch: Parameters<typeof setProvider>[0] = {};
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
    try { take(await setProvider(patch)); saved = "Saved."; if (patch.base_url) listModels(); } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  async function runTest() {
    test = { ok: true, text: "Testing…" };
    try {
      const r = await testProvider();
      test = r.ok ? { ok: true, text: `Connected in ${r.latency_ms} ms.` } : { ok: false, text: `The call failed${r.error ? `: ${r.error}` : "."}` };
    } catch (e) { test = { ok: false, text: refusal(e) }; }
  }
  const keyChanged = async (fn: () => Promise<Provider>) => { take(await fn()); listModels(); };
  onMount(() => { void load(); listModels(); });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if p}
  <section class="bcard" aria-labelledby="pv-ep">
    <h3 id="pv-ep">Endpoint</h3>
    <div class="fields">
      <div class="field"><span class="lbl">Provider</span><span>Anthropic Messages API</span></div>
      <div class="field">
        <label for="pv-base">Base URL <SourceBadge source={p.sources.base_url} /></label>
        <input id="pv-base" type="url" spellcheck="false" bind:value={base} readonly={ro("base_url")} />
      </div>
      <div class="field">
        <span class="lbl">API key <SourceBadge source={p.sources.key} /></span>
        <KeyField info={p.key} label="Instance API key" readOnly={ro("key")} {now} onSave={k => keyChanged(() => setInstanceKey(k))} onClear={() => keyChanged(clearInstanceKey)} />
      </div>
    </div>
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
    {#if models}<p class="note">{models.models.length} models listed by the provider with this key.</p>{/if}
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
{:else if !err}
  <p class="note" role="status">Loading the provider…</p>
{/if}
