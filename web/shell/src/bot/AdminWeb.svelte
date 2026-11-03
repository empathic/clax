<svelte:options css="injected" />

<script lang="ts">
  // Admin › Web (spec 2026-11-02-claxbot-web-tools §8, §9.1): whether
  // ClaxBot may search the web and read pages (two switches), the search
  // provider with its price from the Pricing lines, the per-run caps, which
  // URLs a fetch may take (and the allowlist's hosts), and, on the daemon,
  // the Cloudflare account ID and API token (write-only, as the model key
  // is). Each field shows where its value comes from; one set in
  // `config.toml` or the environment is read-only here. Save sends the
  // fields changed.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type UrlRule, type WebPatch, type WebProvider, type WebSettings, deleteWebToken, getAdminWeb, pricing, putAdminWeb, putWebToken } from "./api";
  import { hosted } from "./me";
  import { UNITS, credits, readOnlySource, webPrices } from "./model";
  import KeyField from "./KeyField.svelte";
  import SourceBadge from "./SourceBadge.svelte";
  import Switch from "./Switch.svelte";

  let { now }: { now?: () => Date } = $props();
  let w = $state<WebSettings | null>(null);
  let err = $state<string | null>(null);
  let saved = $state("");
  let busy = $state(false);
  let local = $state(false);
  let prices = $state<ReturnType<typeof webPrices> | null>(null);
  let searchOn = $state(false);
  let fetchOn = $state(false);
  let provider = $state<WebProvider>("ceramic");
  let rule = $state<UrlRule>("sourced");
  let allow = $state("");
  let gateway = $state("");
  let alias = $state("");
  let limit = $state("");
  let maxSearch = $state("");
  let maxFetch = $state("");
  let account = $state("");
  const PROVIDERS: { id: WebProvider; label: string }[] = [{ id: "ceramic", label: "Ceramic" }, { id: "exa", label: "Exa" }, { id: "linkup", label: "Linkup" }];
  const RULES: { id: UrlRule; label: string }[] = [
    { id: "sourced", label: "Only URLs from the sender, searches and fetched pages" },
    { id: "any_public", label: "Any public URL" },
    { id: "allowlist", label: "Allowlist" },
  ];
  const ro = (field: string) => readOnlySource(w?.sources[field]);
  const price = (p: WebProvider) => { const m = prices?.search[p]; return m === undefined ? "" : ` · ${credits(m)} credits per ${UNITS.kreq}`; };
  const hosts = (text: string) => text.split(/[\s,]+/).map(h => h.trim()).filter(Boolean);

  function take(x: WebSettings) {
    w = x; searchOn = x.search.enabled; fetchOn = x.fetch.enabled; provider = x.search.provider; gateway = x.search.gateway; alias = x.search.byok_alias; limit = String(x.search.limit); rule = x.fetch.url_rule;
    allow = x.fetch.allowlist.join("\n"); maxSearch = String(x.search.max_per_run); maxFetch = String(x.fetch.max_per_run); account = x.cloudflare.account_id;
  }
  // Setting or clearing the token changes the token alone: edits not yet saved stay.
  function takeToken(x: WebSettings) {
    if (w) w = { ...w, cloudflare: { ...w.cloudflare, token: x.cloudflare.token }, sources: x.sources };
  }
  async function save() {
    if (!w || busy) return;
    const patch: WebPatch = {};
    const s: NonNullable<WebPatch["search"]> = {};
    if (!ro("search.enabled") && searchOn !== w.search.enabled) s.enabled = searchOn;
    if (!ro("search.provider") && provider !== w.search.provider) s.provider = provider;
    if (!ro("search.gateway") && gateway.trim() !== w.search.gateway) s.gateway = gateway.trim();
    if (!ro("search.byok_alias") && alias.trim() !== w.search.byok_alias) s.byok_alias = alias.trim();
    if (!ro("search.limit") && Number(limit) !== w.search.limit) {
      const n = Number(limit);
      if (!Number.isInteger(n) || n < 1 || n > 10) { err = "Results per search is a whole number from 1 to 10."; return; }
      s.limit = n;
    }
    const f: NonNullable<WebPatch["fetch"]> = {};
    if (!ro("fetch.enabled") && fetchOn !== w.fetch.enabled) f.enabled = fetchOn;
    if (!ro("fetch.url_rule") && rule !== w.fetch.url_rule) f.url_rule = rule;
    const list = hosts(allow);
    if (!ro("fetch.allowlist") && JSON.stringify(list) !== JSON.stringify(w.fetch.allowlist)) f.allowlist = list;
    for (const [key, text, cur, into] of [["search.max_per_run", maxSearch, w.search.max_per_run, s], ["fetch.max_per_run", maxFetch, w.fetch.max_per_run, f]] as const) {
      if (ro(key) || Number(text) === cur) continue;
      const n = Number(text);
      if (!Number.isInteger(n) || n < 1) { err = `${key.startsWith("search") ? "Searches" : "Fetches"} per run is a whole number, 1 or more.`; return; }
      (into as { max_per_run?: number }).max_per_run = n;
    }
    if (Object.keys(s).length) patch.search = s;
    if (Object.keys(f).length) patch.fetch = f;
    if (local && !ro("cloudflare.account_id") && account.trim() !== w.cloudflare.account_id) patch.cloudflare = { account_id: account.trim() };
    if (!Object.keys(patch).length) { saved = "Nothing changed."; err = null; return; }
    busy = true; err = null; saved = "";
    try { take(await putAdminWeb(patch)); saved = "Saved."; } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  onMount(() => {
    void getAdminWeb().then(take, e => { err = refusal(e); });
    void pricing().then(p => { prices = webPrices(p.lines, p.credit_price_micro_usd); }, () => {});
    void hosted().then(h => { local = !h; });
  });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if w}
  <section class="bcard" aria-labelledby="aw-on">
    <h3 id="aw-on">Web access</h3>
    <div class="fields">
      <div class="field"><Switch checked={searchOn} label="Web search" disabled={ro("search.enabled")} onchange={on => { searchOn = on; }} /> <SourceBadge source={w.sources["search.enabled"]} /></div>
      <div class="field"><Switch checked={fetchOn} label="Web fetch" disabled={ro("fetch.enabled")} onchange={on => { fetchOn = on; }} /> <SourceBadge source={w.sources["fetch.enabled"]} /></div>
    </div>
    <p class="note">Web content can try to steer ClaxBot. It fetches only URLs from the sender's words, searches and pages it read, but which links it follows and what it searches for can still carry data out.</p>
    {#if w.unavailable}<p class="note">Web access is off: {w.unavailable}</p>
    {:else if w.checking}<p class="note">Checking whether web access can run here. Reload in a moment.</p>
    {:else if !w.configured && local}<p class="note">Not configured: web access needs the Cloudflare account ID and API token below.</p>{/if}
  </section>
  <section class="bcard" aria-labelledby="aw-s">
    <h3 id="aw-s">Search</h3>
    <div class="fields">
      <div class="field">
        <label for="aw-provider">Search provider <SourceBadge source={w.sources["search.provider"]} /></label>
        <select id="aw-provider" bind:value={provider} disabled={ro("search.provider")}>
          {#each PROVIDERS as p (p.id)}<option value={p.id}>{p.label}{price(p.id)}</option>{/each}
        </select>
        {#if prices?.fetch !== null && prices?.fetch !== undefined}<small class="note">A page fetched costs {credits(prices.fetch)} credits per {UNITS.kreq}.</small>{/if}
      </div>
      <div class="field">
        <label for="aw-limit">Results per search <SourceBadge source={w.sources["search.limit"]} /></label>
        <input id="aw-limit" type="number" min="1" max="10" step="1" inputmode="numeric" bind:value={limit} readonly={ro("search.limit")} />
      </div>
      <div class="field">
        <label for="aw-gateway">Gateway <SourceBadge source={w.sources["search.gateway"]} /></label>
        <input id="aw-gateway" type="text" spellcheck="false" autocomplete="off" bind:value={gateway} readonly={ro("search.gateway")} />
      </div>
      <div class="field">
        <label for="aw-byok">Provider key alias <SourceBadge source={w.sources["search.byok_alias"]} /></label>
        <input id="aw-byok" type="text" spellcheck="false" autocomplete="off" bind:value={alias} readonly={ro("search.byok_alias")} aria-describedby="aw-byok-note" />
        <small id="aw-byok-note" class="note">With an alias, searches bill the provider key stored on the gateway instead of Cloudflare credits.</small>
      </div>
      <div class="field">
        <label for="aw-max-search">Searches per run <SourceBadge source={w.sources["search.max_per_run"]} /></label>
        <input id="aw-max-search" type="number" min="1" step="1" inputmode="numeric" bind:value={maxSearch} readonly={ro("search.max_per_run")} />
      </div>
    </div>
  </section>
  <section class="bcard" aria-labelledby="aw-f">
    <h3 id="aw-f">Fetch</h3>
    <div class="fields">
      <div class="field">
        <label for="aw-rule">URLs a fetch may take <SourceBadge source={w.sources["fetch.url_rule"]} /></label>
        <select id="aw-rule" bind:value={rule} disabled={ro("fetch.url_rule")}>
          {#each RULES as r (r.id)}<option value={r.id}>{r.label}</option>{/each}
        </select>
        {#if rule === "any_public"}<small class="note" role="note">Any URL ClaxBot writes may be fetched, so a page can make it send data out.</small>{/if}
      </div>
      {#if rule === "allowlist"}
        <div class="field">
          <label for="aw-allow">Allowed hosts <SourceBadge source={w.sources["fetch.allowlist"]} /></label>
          <textarea id="aw-allow" rows="4" spellcheck="false" bind:value={allow} readonly={ro("fetch.allowlist")}></textarea>
          <small class="note">One lowercase hostname per line; <code>*.example.com</code> allows its subdomains.</small>
        </div>
      {/if}
      <div class="field">
        <label for="aw-max-fetch">Fetches per run <SourceBadge source={w.sources["fetch.max_per_run"]} /></label>
        <input id="aw-max-fetch" type="number" min="1" step="1" inputmode="numeric" bind:value={maxFetch} readonly={ro("fetch.max_per_run")} />
      </div>
    </div>
  </section>
  {#if local}
    <section class="bcard" aria-labelledby="aw-cf">
      <h3 id="aw-cf">Cloudflare</h3>
      <div class="fields">
        <div class="field">
          <label for="aw-account">Account ID <SourceBadge source={w.sources["cloudflare.account_id"]} /></label>
          <input id="aw-account" type="text" spellcheck="false" autocomplete="off" bind:value={account} readonly={ro("cloudflare.account_id")} />
        </div>
        <div class="field">
          <span class="lbl">API token <SourceBadge source={w.sources["cloudflare.token"]} /></span>
          <KeyField info={w.cloudflare.token} label="Cloudflare API token" readOnly={ro("cloudflare.token")} {now} onSave={async k => { takeToken(await putWebToken(k)); }} onClear={async () => { takeToken(await deleteWebToken()); }} />
        </div>
      </div>
    </section>
  {/if}
  <div class="acts">
    <button type="button" class="primary" aria-disabled={busy} onclick={save}>{busy ? "Saving…" : "Save"}</button>
    <span role="status">{#if saved}<span class="ok">{saved}</span>{/if}</span>
  </div>
{:else if !err}
  <p class="note" role="status">Loading the web settings…</p>
{/if}
