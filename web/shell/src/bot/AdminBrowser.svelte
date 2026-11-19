<svelte:options css="injected" />

<script lang="ts">
  // Settings, Browser (spec 2026-11-19-claxbot-browser §4.2, §11.1, §11.2):
  // the three capabilities that open a browser on Cloudflare Browser Run, each
  // on its own card with its switch, its per-run cap and who holds it (a
  // switch offers a capability and grants it to no one): a public URL, with
  // the URLs it may open and the risk that carries; the run's clax; content
  // the script supplies. Then the browser's own settings, shared by the three.
  // The Cloudflare account ID and token are Web access's: this page says
  // whether each is set and links there, and names the permission the token
  // needs. Each field shows where its value comes from; one set in
  // `config.toml` or the environment is read-only here. Save sends the
  // fields changed.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type AdminGrant, type AdminPerson, type BrowseRule, type BrowserPatch, type BrowserSettings, adminPeople, getAdminBrowser, getAdminGrants, putAdminBrowser } from "./api";
  import { hosted } from "./me";
  import { credits, readOnlySource } from "./model";
  import CapabilityCard from "./CapabilityCard.svelte";
  import SourceBadge from "./SourceBadge.svelte";
  import Switch from "./Switch.svelte";
  import { holdReveal } from "../settings/first-data";

  type Kind = "url" | "artifact" | "content";
  type Family = "call_seconds" | "idle_seconds" | "max_seconds" | "max_open_per_run" | "max_concurrent";
  let b = $state<BrowserSettings | null>(null);
  let grants = $state<AdminGrant[]>([]);
  let people = $state<AdminPerson[]>([]);
  let err = $state<string | null>(null);
  let saved = $state("");
  let busy = $state(false);
  let local = $state(false);
  let on = $state<Record<Kind, boolean>>({ url: false, artifact: false, content: false });
  let max = $state<Record<Kind, string>>({ url: "", artifact: "", content: "" });
  let rule = $state<BrowseRule>("sender");
  let allow = $state("");
  let family = $state<Record<Family, string>>({ call_seconds: "", idle_seconds: "", max_seconds: "", max_open_per_run: "", max_concurrent: "" });
  const KINDS: { kind: Kind; id: "browser.open.url" | "browser.open.artifact" | "browser.open.content"; title: string; noun: string; switchLabel: string; note: string; cap: string }[] = [
    { kind: "url", id: "browser.open.url", title: "Open a public URL", noun: "public URLs", switchLabel: "Open public URLs", cap: "Sessions on a URL per run", note: "A page can steer ClaxBot, and ClaxBot writes what it types, the code it runs and the URLs it opens: it can type data into a form on a site it may open, and under Any public URL send an artifact's data to any site. Under the other rules a page reaches only the hosts the rule admits. A session on the clax or on supplied content that goes to another site needs this capability too." },
    { kind: "artifact", id: "browser.open.artifact", title: "Open the run's clax", noun: "the run's clax", switchLabel: "Open the run's clax", cap: "Sessions and tests on the clax per run", note: "ClaxBot loads the pages it builds in a browser to look at them and run their saved tests. A page reaches nothing but its own origin, and its API is read-only." },
    { kind: "content", id: "browser.open.content", title: "Open supplied content", noun: "supplied content", switchLabel: "Open supplied content", cap: "Sessions on content per run", note: "ClaxBot renders HTML and files it supplies, to look at a proposed direction or record it. The page reaches nothing but its own origin." },
  ];
  const RULES: { id: BrowseRule; label: string }[] = [
    { id: "sender", label: "Only URLs in the sender's own words" },
    { id: "sourced", label: "Only URLs from the sender, searches and fetched pages" },
    { id: "allowlist", label: "Allowlist" },
    { id: "any_public", label: "Any public URL" },
  ];
  const FAMILY: { key: Family; label: string; lo: number; hi: number; note: string }[] = [
    { key: "call_seconds", label: "Seconds per call", lo: 10, hi: 120, note: "The longest one call may take, 10 to 120 seconds; a person must have the credits for it before a call starts." },
    { key: "idle_seconds", label: "Idle seconds before a session closes", lo: 60, hi: 1200, note: "A session with no call this long is closed: browser time costs while ClaxBot thinks." },
    { key: "max_seconds", label: "Longest session, in seconds", lo: 60, hi: 7200, note: "" },
    { key: "max_open_per_run", label: "Sessions open at once per run", lo: 1, hi: 4, note: "" },
    { key: "max_concurrent", label: "Browsers at once", lo: 1, hi: 50, note: "On this instance, across every run. Workers Free allows 3." },
  ];
  const ro = (field: string) => readOnlySource(b?.sources[field]);
  const hosts = (text: string) => text.split(/[\s,]+/).map(h => h.trim()).filter(Boolean);
  const isSet = (x: boolean) => (x ? "set" : "not set");

  function take(x: BrowserSettings) {
    b = x;
    on = { url: x.url.enabled, artifact: x.artifact.enabled, content: x.content.enabled };
    max = { url: String(x.url.max_per_run), artifact: String(x.artifact.max_per_run), content: String(x.content.max_per_run) };
    rule = x.url.url_rule; allow = x.url.allowlist.join("\n");
    family = { call_seconds: String(x.call_seconds), idle_seconds: String(x.idle_seconds), max_seconds: String(x.max_seconds), max_open_per_run: String(x.max_open_per_run), max_concurrent: String(x.max_concurrent) };
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
    for (const { kind, cap } of KINDS) {
      const g: NonNullable<BrowserPatch["url"]> = {};
      if (!ro(`${kind}.enabled`) && on[kind] !== b[kind].enabled) g.enabled = on[kind];
      if (!ro(`${kind}.max_per_run`)) {
        const n = whole(max[kind], b[kind].max_per_run, 1, Number.MAX_SAFE_INTEGER);
        if (n === null) { err = `${cap} is a whole number, 1 or more.`; saved = ""; return; }
        if (n !== undefined) g.max_per_run = n;
      }
      if (kind === "url") {
        if (!ro("url.url_rule") && rule !== b.url.url_rule) g.url_rule = rule;
        const list = hosts(allow);
        if (!ro("url.allowlist") && JSON.stringify(list) !== JSON.stringify(b.url.allowlist)) g.allowlist = list;
      }
      if (Object.keys(g).length) patch[kind] = g;
    }
    for (const { key, label, lo, hi } of FAMILY) {
      if (ro(key)) continue;
      const n = whole(family[key], b[key], lo, hi);
      if (n === null) { err = `${label} is a whole number from ${lo} to ${hi}.`; saved = ""; return; }
      if (n !== undefined) patch[key] = n;
    }
    if (!Object.keys(patch).length) { saved = "Nothing changed."; err = null; return; }
    busy = true; err = null; saved = "";
    try { take(await putAdminBrowser(patch)); saved = "Saved."; } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  // Shown in Settings, the screen waits for the settings, the grants, the people and whether the daemon is local.
  const release = holdReveal();
  onMount(() => {
    void Promise.allSettled([
      getAdminBrowser().then(take, e => { err = refusal(e); }),
      getAdminGrants().then(g => { grants = g.grants; }, () => {}),
      adminPeople().then(p => { people = p; }, () => {}),
      hosted().then(h => { local = !h; }),
    ]).then(release);
  });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if b}
  {#if b.unavailable}<p class="note">The browser is off: {b.unavailable}</p>{/if}
  {#if local}
    <p class="note">Cloudflare account ID: {isSet(b.cloudflare.account)} · API token: {isSet(b.cloudflare.token)}. <a href="/settings/web#aw-cf">Set them under Web access.</a> The token needs the Browser Rendering – Edit permission, beside Workers AI Read and AI Gateway Read.</p>
  {/if}
  {#snippet card(k: (typeof KINDS)[number], s: BrowserSettings)}
      <div class="fields">
        <div class="field"><Switch checked={on[k.kind]} label={k.switchLabel} disabled={ro(`${k.kind}.enabled`)} onchange={x => { on[k.kind] = x; }} /> <SourceBadge source={s.sources[`${k.kind}.enabled`]} /></div>
      </div>
      <p class="note">{k.note}{#if s[k.kind].price_milli_per_minute != null}{` Browser time costs ${credits(s[k.kind].price_milli_per_minute ?? 0)} credits a minute, by the millisecond.`}{/if}</p>
      <div class="fields">
        {#if k.kind === "url"}
          <div class="field">
            <label for="ab-rule">URLs a session may open <SourceBadge source={s.sources["url.url_rule"]} /></label>
            <select id="ab-rule" bind:value={rule} disabled={ro("url.url_rule")}>
              {#each RULES as r (r.id)}<option value={r.id}>{r.label}</option>{/each}
            </select>
            {#if rule === "any_public"}<small class="note" role="note">Any public URL ClaxBot writes may be opened, so a page can make it send data out.</small>{/if}
          </div>
          {#if rule === "allowlist"}
            <div class="field">
              <label for="ab-allow">Allowed hosts <SourceBadge source={s.sources["url.allowlist"]} /></label>
              <textarea id="ab-allow" rows="4" spellcheck="false" bind:value={allow} readonly={ro("url.allowlist")}></textarea>
              <small class="note">One lowercase hostname per line; <code>*.example.com</code> allows its subdomains.</small>
            </div>
          {/if}
        {/if}
        <div class="field">
          <label for={`ab-max-${k.kind}`}>{k.cap} <SourceBadge source={s.sources[`${k.kind}.max_per_run`]} /></label>
          <input id={`ab-max-${k.kind}`} type="number" min="1" step="1" inputmode="numeric" bind:value={max[k.kind]} readonly={ro(`${k.kind}.max_per_run`)} />
        </div>
      </div>
      <CapabilityCard capability={k.id} label={k.noun} {grants} {people} onchange={g => { grants = g; }} />
  {/snippet}
  <section class="bcard" aria-labelledby="ab-url"><h3 id="ab-url">{KINDS[0].title}</h3>{@render card(KINDS[0], b)}</section>
  <section class="bcard" aria-labelledby="ab-artifact"><h3 id="ab-artifact">{KINDS[1].title}</h3>{@render card(KINDS[1], b)}</section>
  <section class="bcard" aria-labelledby="ab-content"><h3 id="ab-content">{KINDS[2].title}</h3>{@render card(KINDS[2], b)}</section>
  <section class="bcard" aria-labelledby="ab-family">
    <h3 id="ab-family">Browser sessions</h3>
    <div class="fields">
      {#each FAMILY as f (f.key)}
        <div class="field">
          <label for={`ab-${f.key}`}>{f.label} <SourceBadge source={b.sources[f.key]} /></label>
          <input id={`ab-${f.key}`} type="number" min={f.lo} max={f.hi} step="1" inputmode="numeric" bind:value={family[f.key]} readonly={ro(f.key)} />
          {#if f.note}<small class="note">{f.note}</small>{/if}
        </div>
      {/each}
    </div>
  </section>
  <div class="acts">
    <button type="button" class="primary" aria-disabled={busy} onclick={save}>{busy ? "Saving…" : "Save"}</button>
    <span role="status">{#if saved}<span class="ok">{saved}</span>{/if}</span>
  </div>
{:else if !err}
  <p class="note" role="status">Loading the browser settings…</p>
{/if}
