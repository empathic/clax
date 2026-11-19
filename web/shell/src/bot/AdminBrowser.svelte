<svelte:options css="injected" />

<script lang="ts">
  // Settings, Browser (spec 2026-11-19-claxbot-browser §9.4, §10.1): whether
  // ClaxBot may test the pages it builds in Cloudflare Browser Run, the
  // per-run and per-test caps, the browsers open at once on the instance,
  // and, separately, whether it may browse public pages and which URLs it
  // may open, with the risk that carries beside the switch. The Cloudflare
  // account ID and token are Web access's: this page says whether each is
  // set and links there, and names the permission the token needs. Each
  // field shows where its value comes from; one set in `config.toml` or the
  // environment is read-only here. Save sends the fields changed.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type BrowseRule, type BrowserPatch, type BrowserSettings, getAdminBrowser, pricing, putAdminBrowser } from "./api";
  import { hosted } from "./me";
  import { credits, perUnitMilli, readOnlySource } from "./model";
  import SourceBadge from "./SourceBadge.svelte";
  import Switch from "./Switch.svelte";
  import { holdReveal } from "../settings/first-data";

  let b = $state<BrowserSettings | null>(null);
  let err = $state<string | null>(null);
  let saved = $state("");
  let busy = $state(false);
  let local = $state(false);
  let hourMilli = $state<number | null>(null);
  let testOn = $state(false);
  let maxTests = $state("");
  let seconds = $state("");
  let concurrent = $state("");
  let browseOn = $state(false);
  let rule = $state<BrowseRule>("sender");
  let allow = $state("");
  let maxBrowses = $state("");
  const RULES: { id: BrowseRule; label: string }[] = [
    { id: "sender", label: "Only URLs in the sender's own words" },
    { id: "allowlist", label: "Allowlist" },
    { id: "any_public", label: "Any public URL" },
  ];
  const ro = (field: string) => readOnlySource(b?.sources[field]);
  const hosts = (text: string) => text.split(/[\s,]+/).map(h => h.trim()).filter(Boolean);
  const isSet = (on: boolean) => (on ? "set" : "not set");

  function take(x: BrowserSettings) {
    b = x; testOn = x.test.enabled; maxTests = String(x.test.max_per_run); seconds = String(x.test.seconds); concurrent = String(x.max_concurrent);
    browseOn = x.browse.enabled; rule = x.browse.url_rule; allow = x.browse.allowlist.join("\n"); maxBrowses = String(x.browse.max_per_run);
  }
  /** `text` as a whole number from `lo` to `hi` when it differs from `cur`; `undefined` when unchanged; `null` when it is no such number. */
  function whole(text: string, cur: number, lo: number, hi: number): number | undefined | null {
    const n = Number(text);
    if (n === cur) return undefined;
    return Number.isInteger(n) && n >= lo && n <= hi ? n : null;
  }
  async function save() {
    if (!b || busy) return;
    const patch: BrowserPatch = {};
    const t: NonNullable<BrowserPatch["test"]> = {};
    const w: NonNullable<BrowserPatch["browse"]> = {};
    if (!ro("test.enabled") && testOn !== b.test.enabled) t.enabled = testOn;
    if (!ro("browse.enabled") && browseOn !== b.browse.enabled) w.enabled = browseOn;
    if (!ro("browse.url_rule") && rule !== b.browse.url_rule) w.url_rule = rule;
    const list = hosts(allow);
    if (!ro("browse.allowlist") && JSON.stringify(list) !== JSON.stringify(b.browse.allowlist)) w.allowlist = list;
    const numbers = [
      ["test.max_per_run", maxTests, b.test.max_per_run, 1, Number.MAX_SAFE_INTEGER, "Tests per run is a whole number, 1 or more."],
      ["test.seconds", seconds, b.test.seconds, 10, 120, "Seconds per test is a whole number from 10 to 120."],
      ["browse.max_per_run", maxBrowses, b.browse.max_per_run, 1, Number.MAX_SAFE_INTEGER, "Browses per run is a whole number, 1 or more."],
      ["max_concurrent", concurrent, b.max_concurrent, 1, 50, "Browsers at once is a whole number from 1 to 50."],
    ] as const;
    for (const [key, text, cur, lo, hi, words] of numbers) {
      if (ro(key)) continue;
      const n = whole(text, cur, lo, hi);
      if (n === null) { err = words; saved = ""; return; }
      if (n === undefined) continue;
      if (key === "test.max_per_run") t.max_per_run = n;
      else if (key === "test.seconds") t.seconds = n;
      else if (key === "browse.max_per_run") w.max_per_run = n;
      else patch.max_concurrent = n;
    }
    if (Object.keys(t).length) patch.test = t;
    if (Object.keys(w).length) patch.browse = w;
    if (!Object.keys(patch).length) { saved = "Nothing changed."; err = null; return; }
    busy = true; err = null; saved = "";
    try { take(await putAdminBrowser(patch)); saved = "Saved."; } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  // Shown in Settings, the screen waits for the settings, the browser line's price and whether the daemon is local.
  const release = holdReveal();
  onMount(() => {
    void Promise.allSettled([
      getAdminBrowser().then(take, e => { err = refusal(e); }),
      pricing().then(p => { const l = p.lines.find(x => x.id === "browser.time"); hourMilli = l ? perUnitMilli({ ...l, cost_micro_usd: l.cost_micro_usd * 3600, fixed_credits_milli: l.fixed_credits_milli == null ? null : l.fixed_credits_milli * 3600 }, p.credit_price_micro_usd) : null; }, () => {}),
      hosted().then(h => { local = !h; }),
    ]).then(release);
  });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if b}
  <section class="bcard" aria-labelledby="ab-on">
    <h3 id="ab-on">Browser testing</h3>
    <div class="fields">
      <div class="field"><Switch checked={testOn} label="Browser testing" disabled={ro("test.enabled")} onchange={on => { testOn = on; }} /> <SourceBadge source={b.sources["test.enabled"]} /></div>
    </div>
    <p class="note">ClaxBot loads the pages it builds in Cloudflare Browser Run to test them. A test reaches nothing but its page: every request to another origin is refused, and the page's API is read-only.{#if hourMilli !== null}{` Browser time costs ${credits(hourMilli)} credits per browser hour, by the millisecond.`}{/if}</p>
    {#if b.unavailable}<p class="note">Browser testing is off: {b.unavailable}</p>{/if}
    {#if local}
      <p class="note">Cloudflare account ID: {isSet(b.cloudflare.account)} · API token: {isSet(b.cloudflare.token)}. <a href="/settings/web#aw-cf">Set them under Web access.</a> The token needs the Browser Rendering – Edit permission, beside Workers AI Read and AI Gateway Read.</p>
    {/if}
    <div class="fields">
      <div class="field">
        <label for="ab-max-tests">Tests per run <SourceBadge source={b.sources["test.max_per_run"]} /></label>
        <input id="ab-max-tests" type="number" min="1" step="1" inputmode="numeric" bind:value={maxTests} readonly={ro("test.max_per_run")} />
      </div>
      <div class="field">
        <label for="ab-seconds">Seconds per test <SourceBadge source={b.sources["test.seconds"]} /></label>
        <input id="ab-seconds" type="number" min="10" max="120" step="1" inputmode="numeric" bind:value={seconds} readonly={ro("test.seconds")} aria-describedby="ab-seconds-note" />
        <small id="ab-seconds-note" class="note">The longest a test may run, 10 to 120 seconds; a person must have the credits for it before a test starts.</small>
      </div>
      <div class="field">
        <label for="ab-concurrent">Browsers at once <SourceBadge source={b.sources["max_concurrent"]} /></label>
        <input id="ab-concurrent" type="number" min="1" max="50" step="1" inputmode="numeric" bind:value={concurrent} readonly={ro("max_concurrent")} aria-describedby="ab-concurrent-note" />
        <small id="ab-concurrent-note" class="note">On this instance, across every run. Workers Free allows 3.</small>
      </div>
    </div>
  </section>
  <section class="bcard" aria-labelledby="ab-b">
    <h3 id="ab-b">Browsing public pages</h3>
    <div class="fields">
      <div class="field"><Switch checked={browseOn} label="Browse public pages" disabled={ro("browse.enabled")} onchange={on => { browseOn = on; }} /> <SourceBadge source={b.sources["browse.enabled"]} /></div>
    </div>
    <p class="note">A page can steer ClaxBot, and ClaxBot writes what it types, the code it runs and the URLs it opens: it can type data into a form on a site it may open, and under Any public URL send an artifact's data to any site. Under the other rules a browsed page reaches only the hosts the rule admits, so what a page loads from elsewhere is left out. Browsing also needs each person's web switch on.</p>
    <div class="fields">
      <div class="field">
        <label for="ab-rule">URLs a browse may open <SourceBadge source={b.sources["browse.url_rule"]} /></label>
        <select id="ab-rule" bind:value={rule} disabled={ro("browse.url_rule")}>
          {#each RULES as r (r.id)}<option value={r.id}>{r.label}</option>{/each}
        </select>
        {#if rule === "any_public"}<small class="note" role="note">Any public URL ClaxBot writes may be opened, so a page can make it send data out.</small>{/if}
      </div>
      {#if rule === "allowlist"}
        <div class="field">
          <label for="ab-allow">Allowed hosts <SourceBadge source={b.sources["browse.allowlist"]} /></label>
          <textarea id="ab-allow" rows="4" spellcheck="false" bind:value={allow} readonly={ro("browse.allowlist")}></textarea>
          <small class="note">One lowercase hostname per line; <code>*.example.com</code> allows its subdomains.</small>
        </div>
      {/if}
      <div class="field">
        <label for="ab-max-browse">Browses per run <SourceBadge source={b.sources["browse.max_per_run"]} /></label>
        <input id="ab-max-browse" type="number" min="1" step="1" inputmode="numeric" bind:value={maxBrowses} readonly={ro("browse.max_per_run")} />
      </div>
    </div>
  </section>
  <div class="acts">
    <button type="button" class="primary" aria-disabled={busy} onclick={save}>{busy ? "Saving…" : "Save"}</button>
    <span role="status">{#if saved}<span class="ok">{saved}</span>{/if}</span>
  </div>
{:else if !err}
  <p class="note" role="status">Loading the browser settings…</p>
{/if}
