<svelte:options css="injected" />

<script lang="ts">
  // `/settings` (spec 2026-10-21-claxbot D4, §8; crates/clax-server/src/
  // routes/settings.rs), the localhost daemon's: every config field, by
  // section, with the value in effect and where it comes from (default,
  // `config.toml`, the environment), a value the daemon takes at its next
  // start marked "after restart", and a field's note. A field the
  // environment sets is read-only here, as is `bin`. Model fields are the
  // provider's model selector (↻ and a custom ID), with Test connection
  // beside them. Save writes the fields changed to `config.toml` and shows
  // what the daemon reads back; a file that did not read when the daemon
  // started, and tables whose values do not check, are said at the top. A
  // hosted daemon has no settings screen (404), and says so. Below the
  // fields, Confirmations: whether confirming requires Touch ID or a
  // passkey, and the passkeys registered (`q/PasskeySettings.svelte`).
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type ProviderModels, type Settings, providerModels, saveSettings, settings, testProvider } from "./api";
  import { asText, fromText, isModel, sections } from "./model";
  import BotPage from "./BotPage.svelte";
  import ModelSelect from "./ModelSelect.svelte";
  import SourceBadge from "./SourceBadge.svelte";
  import Switch from "./Switch.svelte";
  import PasskeySettings from "../q/PasskeySettings.svelte";

  let s = $state<Settings | null>(null);
  let none = $state(false);
  let err = $state<string | null>(null);
  let saved = $state("");
  let busy = $state(false);
  // The form's values by field name, and the fields changed.
  let draft = $state<Record<string, unknown>>({});
  // The provider's models for the model fields, and why they are not listed.
  let models = $state<ProviderModels | null>(null);
  let why = $state<string | null>(null);
  let test = $state<{ ok: boolean; text: string } | null>(null);
  const secs = $derived(s ? sections(s.fields) : []);
  const original = (k: string) => s?.fields.find(f => f.key === k)?.value;
  const changed = $derived(s ? Object.keys(draft).filter(k => JSON.stringify(draft[k]) !== JSON.stringify(original(k))) : []);
  const restart = $derived(changed.some(k => s?.fields.find(f => f.key === k)?.restart_required));
  const hasModels = $derived(!!s?.fields.some(f => isModel(f.key)));
  const take = (x: Settings) => { s = x; draft = Object.fromEntries(x.fields.map(f => [f.key, f.value])); };
  const id = (k: string) => `set-${k.replace(/[^a-z0-9]/gi, "-")}`;
  function listModels() {
    why = "Listing the provider's models…";
    void providerModels().then(m => { models = m; why = null; }, e => { models = null; why = `The models could not be listed: ${refusal(e)}`; });
  }
  async function runTest() {
    test = { ok: true, text: "Testing…" };
    try {
      const r = await testProvider();
      test = r.ok ? { ok: true, text: `Connected in ${r.latency_ms} ms.` } : { ok: false, text: `The call failed${r.error ? `: ${r.error}` : "."}` };
    } catch (e) { test = { ok: false, text: refusal(e) }; }
  }
  async function save() {
    if (!s || busy || !changed.length) return;
    busy = true; err = null; saved = ""; test = null;
    const set = Object.fromEntries(changed.map(k => [k, draft[k]]));
    try {
      const r = await saveSettings(set);
      take(r);
      const n = r.written?.length ?? changed.length;
      saved = `Saved ${n} to config.toml.${r.restart_required ? " Restart Clax for them to take effect." : ""}`;
    } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  onMount(() => {
    document.title = "Settings · Clax";
    void settings().then(x => { if (x) { take(x); if (x.fields.some(f => isModel(f.key))) listModels(); } else none = true; }, e => { err = refusal(e); });
  });
</script>

<BotPage title="Settings" lede="This Clax's configuration: config.toml in its home, overridden by CLAX_* environment variables. Each field says where its value comes from; a field the environment sets is read-only here.">
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  {#if none}
    <p class="note">This Clax has no settings screen: it is configured where it is hosted. Admins set ClaxBot's provider and prices under <a href="/admin">Admin</a>.</p>
  {:else if s}
    {#if s.started_with_invalid_config}
      <div class="sbad" role="alert"><p><b>Clax started without its config.toml:</b> {s.start_error ?? "the file did not read"}. The values below are the defaults until it starts again with a file that reads.</p></div>
    {/if}
    {#each s.problems as p (p.table)}
      <p class="err" role="alert">[{p.table}] is not used: {p.message}</p>
    {/each}
    <p class="note">File: <code>{s.file}</code></p>
    {#each secs as sec (sec.name)}
      <section class="bcard" aria-labelledby={`sec-${sec.name}`}>
        <h3 id={`sec-${sec.name}`}>[{sec.name}]</h3>
        <div class="fields">
          {#each sec.fields as f (f.key)}
            {@const ro = !f.editable}
            <div class="field">
              <span class="lbl" id={`${id(f.key)}-l`}>{f.label} <SourceBadge source={f.source} {ro} />{#if f.pending !== undefined}<span class="pend">after restart: {asText(f.pending) || "unset"}</span>{/if}</span>
              {#if isModel(f.key)}
                <ModelSelect id={id(f.key)} label={f.label} hideLabel value={asText(draft[f.key])} models={ro ? [] : models?.models ?? null} why={ro ? null : why} readOnly={ro} onRefresh={listModels} onChange={v => { draft = { ...draft, [f.key]: v }; }} />
              {:else if typeof f.value === "boolean"}
                <Switch checked={draft[f.key] === true} label={f.key} hideLabel disabled={ro} onchange={on => { draft = { ...draft, [f.key]: on }; }} />
              {:else}
                <input id={id(f.key)} aria-labelledby={`${id(f.key)}-l`} type={typeof f.value === "number" ? "number" : "text"} spellcheck="false" readonly={ro} value={asText(draft[f.key])}
                  oninput={e => { draft = { ...draft, [f.key]: fromText(e.currentTarget.value, f.value) }; }} />
              {/if}
              {#if f.note}<small>{f.note}</small>{/if}
              {#if f.restart_required && !ro}<small>Takes effect when Clax restarts</small>{/if}
              {#if f.env && !ro}<small>Or set {f.env}</small>{/if}
            </div>
          {/each}
        </div>
      </section>
    {/each}
    <div class="acts">
      <button type="button" class="primary" aria-disabled={busy || !changed.length} onclick={save}>{busy ? "Saving…" : "Save to config.toml"}</button>
      {#if hasModels}<button type="button" onclick={runTest}>Test connection</button>{/if}
      <span role="status">{#if test}<span class={test.ok ? "ok" : "err"}>{test.text}</span>{:else if saved}<span class="ok">{saved}</span>{:else if changed.length}<span class="note">{changed.length} changed{restart ? ", takes effect after a restart" : ""}</span>{/if}</span>
    </div>
    <PasskeySettings />
  {:else if !err}
    <p class="note" role="status">Loading the settings…</p>
  {/if}
</BotPage>

<style>
  :global {
    .bpg .pend { font: 500 11.5px/1.5 var(--font); color: var(--tag-yellow-fg); background: var(--tag-yellow-bg); border: 1px solid var(--tag-yellow-bd); border-radius: var(--radius-xs); padding: 0 6px; overflow-wrap: anywhere; }
    .bpg .sbad { padding: 10px 14px; border-radius: var(--radius-sm); background: var(--danger-tint); border: 1px solid var(--danger); font-size: 13.5px; }
    .bpg .sbad p { margin: 0; }
    .bpg .note code { overflow-wrap: anywhere; }
  }
</style>
