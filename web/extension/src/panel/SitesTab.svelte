<script lang="ts">
  // The side panel's Sites tab, the owner's (spec 2026-10-05 §7.2): every
  // site Clax has live pages of, the most recently active first, whichever
  // page the tab shows, of a site or not. Each site shows its name,
  // its addresses, and how many pages and threads it has. Each address of a
  // joined site can be split off; "Join with…" joins another site, picked
  // from the list, into this one, in batches as SiteTools' join does,
  // asking Chrome first (under the click) for every origin of the site the
  // join makes; a site with a join not finished offers to continue it.
  // The list is asked for again after every change, and when the origins of
  // the tab's own site change (a join or split made elsewhere of another
  // site shows only when the tab is opened again). A list that could not be
  // read says why, with Retry.
  import { untrack } from "svelte";
  import type { SiteChoice } from "../messages";
  import { type Step, host, plural, runJoin } from "./site-actions";

  type Ask = { t: "site-join"; origin: string; with: string } | { t: "site-split"; origin: string };
  let { sites, failed = null, siteOrigins = "", request, list, permit }: {
    sites: SiteChoice[] | null;
    /** Why the list could not be read; null while it could. */
    failed?: { code: string; message: string } | null;
    /** The origins of the tab's own site, as one string. */
    siteOrigins?: string;
    request(m: Ask): Promise<Step>; list(): void;
    /** Asks Chrome for `origins` (under the person's click): whether it allowed them. */
    permit(origins: string[]): Promise<boolean>;
  } = $props();
  /** The site picked in each site's "Join with…", by the site's key. */
  let picks = $state<Record<string, string>>({});
  let busy = $state<string | null>(null);
  let note = $state<{ text: string; bad: boolean } | null>(null);
  let alive = true;
  $effect(() => () => { alive = false; });
  // On first showing, and whenever the tab's own site changes.
  $effect(() => { void siteOrigins; untrack(() => list()); });

  /** Joins `other` into `site`; without `other`, finishes the join `site` has not finished, which needs no permission. */
  async function join(site: SiteChoice, other: SiteChoice | null): Promise<void> {
    if (busy) return;
    const all = [...site.origins, ...(other?.origins ?? [])];
    // Asked before any await, so the click still counts as the gesture Chrome needs.
    const allowed = permit(all);
    // A join not finished is repeated from an origin of the site that is not its key.
    const origin = other?.name ?? site.origins.find(o => o !== site.key);
    if (!origin) return;
    const names = other ? [other.name, site.name] : [origin, site.key];
    busy = `Joining ${host(names[0])}`;
    note = null;
    try {
      const r = await runJoin({ all, allowed, continuing: !other, step: () => request({ t: "site-join", origin, with: site.key }),
        left: n => { busy = `Joining ${host(names[0])}… ${plural(n, "thread")} left`; }, alive: () => alive });
      if (!r) return;
      const unfollowed = r.denied ? ` Chrome was not allowed access to ${all.map(host).join(", ")}, so Clax will not follow a tab there.` : "";
      note = { text: `Joined ${host(names[0])} and ${host(names[1])}: one site${r.moved ? `, ${plural(r.moved, "thread")} merged` : ""}.${unfollowed}`, bad: false };
      picks = { ...picks, [site.key]: "" };
    } catch (e) {
      note = { text: e instanceof Error ? e.message : String(e), bad: true };
    } finally {
      busy = null;
      if (alive) list();
    }
  }

  async function split(o: string): Promise<void> {
    if (busy) return;
    busy = `Splitting ${host(o)} off`;
    note = null;
    try {
      await request({ t: "site-split", origin: o });
      note = { text: `${host(o)} is a site of its own again. What the site has kept stays with it.`, bad: false };
    } catch (e) {
      note = { text: e instanceof Error ? e.message : String(e), bad: true };
    } finally {
      busy = null;
      if (alive) list();
    }
  }
</script>

<section class="sites" aria-label="Sites">
  <p class="hint">Addresses that are one site share their pages and comments. Join a dev server on another port to the site it serves, or split an address off.</p>
  {#if busy}<p class="small" role="status">{busy}…</p>{:else if note}<p class={note.bad ? "err" : "small"} role="status">{note.text}</p>{/if}
  {#if failed}
    <div class="failed" role="alert"><p>Clax could not list the sites: {failed.message}</p><button type="button" onclick={() => list()}>Retry</button></div>
  {/if}
  {#if sites === null}
    {#if !failed}<p class="hint">Loading…</p>{/if}
  {:else if !sites.length}
    <p class="hint">No site has live pages yet.</p>
  {:else}
    <ul class="list">
      {#each sites as site (site.key)}
        {@const others = sites.filter(c => c.key !== site.key)}
        {@const chosen = others.find(c => c.key === picks[site.key]) ?? null}
        <li class="site">
          <h2 title={site.name}>{host(site.name)}</h2>
          <p class="small">{plural(site.pages, "page")} · {plural(site.threads, "thread")}</p>
          {#if site.joining > 0}
            <div class="suggest" role="group" aria-label={`Join of ${host(site.name)} not finished`}>
              <p>The join is not finished: {plural(site.joining, "thread")} left to merge here.</p>
              <div class="row"><button type="button" class="primary" disabled={!!busy} onclick={() => join(site, null)}>Continue joining</button></div>
            </div>
          {/if}
          <ul class="origins">
            {#each site.origins as o (o)}
              <li>
                <span class="o" title={o}>{host(o)}</span>
                {#if site.origins.length > 1}<button type="button" class="ghost" disabled={!!busy} aria-label={`Split ${host(o)} off`} onclick={() => split(o)}>Split off</button>{/if}
              </li>
            {/each}
          </ul>
          {#if others.length}
            <div class="join">
              <select aria-label={`Join ${host(site.name)} with`} value={picks[site.key] ?? ""} onchange={e => (picks = { ...picks, [site.key]: e.currentTarget.value })}>
                <option value="">Join with…</option>
                {#each others as c (c.key)}<option value={c.key}>{host(c.name)}{c.origins.length > 1 ? ` (+${c.origins.length - 1})` : ""}</option>{/each}
              </select>
              <button type="button" class="primary" disabled={!chosen || !!busy} aria-label={`Join ${host(site.name)}`} onclick={() => chosen && join(site, chosen)}>Join</button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .sites { padding: 4px var(--gutter) 14px; }
  .hint, .small { margin: 8px 0; font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
  .err { margin: 8px 0; font-size: 12.5px; color: var(--danger); overflow-wrap: anywhere; }
  .failed { display: flex; gap: 10px; align-items: center; margin: 8px 0; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--danger-tint); }
  .failed p { flex: 1; margin: 0; font-size: 12.5px; overflow-wrap: anywhere; }
  .list { margin: 0; padding: 0; list-style: none; }
  .site { padding: 10px 0; border-top: 1px solid var(--border); }
  .site h2 { margin: 0; font: 600 13.5px/1.3 var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .site .small { margin: 2px 0 6px; }
  .suggest { margin: 6px 0; padding: 8px 10px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); background: var(--card); }
  .suggest p { margin: 0 0 6px; font-size: 12.5px; overflow-wrap: anywhere; }
  .origins { margin: 4px 0 8px; padding: 0; list-style: none; }
  .origins li { display: flex; align-items: center; gap: 8px; padding: 3px 0 3px 2px; }
  .o { flex: 1; min-width: 0; font: 12.5px/1.5 var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .origins button { min-height: 28px; padding: 2px 8px; font-size: 12.5px; }
  .join { display: flex; gap: 6px; }
  .join select { flex: 1; min-width: 0; font-size: 13px; }
  .row { display: flex; justify-content: flex-end; gap: 6px; }
</style>
