<script lang="ts">
  // Joined sites in the side panel (spec 2026-10-05 §7.2, owner decisions
  // 2026-10-06): when the tab's origin joined no site and Clax found another
  // of its host family with the same paths or title, "Looks like
  // localhost:7702 — same app?" with Join, Not now and Never; nothing joins
  // unless the person says so. Under "Addresses": the site's origins, each
  // of which can be split off again, and "Same app as…", which joins the
  // tab's origin to a site the person picks. A join runs in batches as a
  // merge does, repeated while the daemon says threads remain. Joining asks
  // Chrome for the other origins first (under the click), so a thread of the
  // site can open on any of them with Clax staying on.
  import type { SiteChoice, SiteView, Suggestion } from "../messages";

  type Step = { moved: number; remaining: number };
  type Ask = { t: "join"; origin: string; with: string } | { t: "split"; origin: string };
  let { site, origin, suggestion, sites, request, answer, list, permit, banner = false }: {
    site: SiteView | null; origin: string; suggestion: Suggestion | null; sites: SiteChoice[] | null;
    /** The suggestion's banner, at the panel's top; else the site's tools ("Addresses"). */
    banner?: boolean;
    request(m: Ask): Promise<Step>; answer(withOrigin: string, a: "never" | "later"): void; list(): void;
    /** Asks Chrome for `origins` (under the person's click): whether it allowed them. */
    permit(origins: string[]): Promise<boolean>;
  } = $props();
  /** The most batches one join sends (200 threads each). */
  const MAX_BATCHES = 1000;
  const host = (o: string) => o.replace(/^\w+:\/\//, "");
  const info = $derived(site?.site ?? null);
  const joined = $derived(!!info?.joined);
  const mine = $derived(info?.origins.map(o => o.origin) ?? [origin]);
  /** The sites the tab's origin may join: every other one. */
  const choices = $derived((sites ?? []).filter(c => !c.origins.some(o => mine.includes(o))));
  let pick = $state("");
  let busy = $state<string | null>(null);
  let note = $state<{ text: string; bad: boolean } | null>(null);
  let alive = true;
  $effect(() => () => { alive = false; });
  const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

  async function join(target: string, all: string[]): Promise<void> {
    if (busy) return;
    // Asked before any await, so the click still counts as the gesture Chrome needs.
    const allowed = permit(all);
    busy = `Joining ${host(target)}`;
    note = null;
    let moved = 0;
    let left = Infinity;
    try {
      if (!(await allowed)) throw new Error(`Chrome was not allowed access to ${all.map(host).join(", ")}, so Clax cannot follow the site there.`);
      for (let i = 0; ; i++) {
        if (!alive) return;
        const r = await request({ t: "join", origin, with: target });
        moved += r.moved;
        if (!r.remaining) break;
        busy = `Joining ${host(target)}… ${plural(r.remaining, "thread")} left`;
        if (!r.moved || r.remaining >= left || i + 1 >= MAX_BATCHES) throw new Error(`Stopped with ${plural(r.remaining, "thread")} left to merge. Join again later to finish.`);
        left = r.remaining;
      }
      note = { text: `Joined ${host(origin)} and ${host(target)}: one site${moved ? `, ${plural(moved, "thread")} merged` : ""}.`, bad: false };
      pick = "";
    } catch (e) {
      note = { text: e instanceof Error ? e.message : String(e), bad: true };
    } finally {
      busy = null;
    }
  }

  async function split(o: string): Promise<void> {
    if (busy) return;
    busy = `Splitting ${host(o)} off`;
    note = null;
    try {
      await request({ t: "split", origin: o });
      note = { text: `${host(o)} is a site of its own again. What the site has kept stays with it.`, bad: false };
    } catch (e) {
      note = { text: e instanceof Error ? e.message : String(e), bad: true };
    } finally {
      busy = null;
    }
  }
  const chosen = $derived(choices.find(c => c.key === pick) ?? null);
</script>

{#snippet status()}
  {#if busy}<p class="small" role="status">{busy}…</p>{:else if note}<p class={note.bad ? "err" : "small"} role="status">{note.text}</p>{/if}
{/snippet}
{#if banner}
  {#if suggestion && (!joined || busy || note)}
    <div class="suggest" role="group" aria-label="Same app?">
      {#if !joined}
        <p>Looks like <strong>{host(suggestion.origin)}</strong> — same app?
          <span class="why">{suggestion.reason === "title" ? "It has a page with this title." : `It has comments on ${suggestion.path ?? "the same paths"}.`}</span></p>
        <div class="row">
          <button type="button" class="primary" disabled={!!busy} onclick={() => join(suggestion.origin, suggestion.origins)}>Join</button>
          <button type="button" class="ghost" disabled={!!busy} onclick={() => answer(suggestion.origin, "later")}>Not now</button>
          <button type="button" class="ghost" disabled={!!busy} onclick={() => answer(suggestion.origin, "never")}>Never</button>
        </div>
      {/if}
      {@render status()}
    </div>
  {/if}
{:else}
{#if (info?.joining ?? 0) > 0}
  {@const other = mine.find(o => o !== origin)}
  <div class="suggest" role="group" aria-label="Join not finished">
    <p>The join is not finished: {plural(info?.joining ?? 0, "thread")} left to merge here.</p>
    {#if other}<div class="row"><button type="button" class="primary" disabled={!!busy} onclick={() => join(other, mine)}>Continue joining</button></div>{/if}
  </div>
{/if}
<details class="addresses" ontoggle={e => { if (e.currentTarget.open) list(); }}>
  <summary><h2>Addresses{#if joined}<span class="count">{mine.length}</span>{/if}</h2></summary>
  {#if joined}
    <p class="hint">These addresses are one site: their pages and comments are shared, and an agent watching one hears them all.</p>
    <ul class="origins">
      {#each mine as o (o)}
        <li>
          <span class="o" title={o}>{host(o)}</span>{#if o === origin}<span class="small">this tab</span>{/if}
          <button type="button" class="ghost" disabled={!!busy} aria-label={`Split ${host(o)} off`} onclick={() => split(o)}>Split off</button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="hint">If {host(origin)} is the same app as another address (a dev server on another port), join them to share pages and comments.</p>
  {/if}
  <label class="field"><span>Same app as…</span>
    <select aria-label="Same app as" value={pick} onchange={e => (pick = e.currentTarget.value)}>
      <option value="">{sites === null ? "Loading…" : choices.length ? "Choose an address" : "No other address yet"}</option>
      {#each choices as c (c.key)}<option value={c.key}>{host(c.name)}{c.origins.length > 1 ? ` (+${c.origins.length - 1})` : ""}</option>{/each}
    </select>
  </label>
  <div class="row"><button type="button" class="primary" disabled={!chosen || !!busy} onclick={() => chosen && join(chosen.name, chosen.origins)}>Join</button></div>
  {@render status()}
</details>
{/if}

<style>
  .suggest { margin: 12px var(--gutter) 0; padding: 10px 12px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); background: var(--card); }
  .suggest p { margin: 0 0 8px; font-size: 13px; overflow-wrap: anywhere; }
  .why { display: block; color: var(--muted); font-size: 12.5px; }
  .addresses { padding: 4px var(--gutter) 14px; border-top: 1px solid var(--border); }
  summary { list-style: none; cursor: pointer; }
  summary::-webkit-details-marker { display: none; }
  summary h2 { display: flex; align-items: center; gap: 8px; margin: 12px 2px 4px; font: 600 14px/1.2 var(--font); }
  summary h2::before { content: "▸"; color: var(--muted); font-size: 11px; }
  details[open] summary h2::before { content: "▾"; }
  .count { font-weight: 400; color: var(--muted); font-size: 12.5px; }
  .hint, .small { margin: 6px 0; font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
  .err { margin: 6px 0; font-size: 12.5px; color: var(--danger); overflow-wrap: anywhere; }
  .origins { margin: 4px 0 8px; padding: 0; list-style: none; }
  .origins li { display: flex; align-items: center; gap: 8px; padding: 4px 0 4px 2px; border-bottom: 1px solid var(--border); }
  .o { flex: 1; min-width: 0; font: 12.5px/1.5 var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .origins button { min-height: 28px; padding: 2px 8px; font-size: 12.5px; }
  .field { display: grid; gap: 4px; margin: 8px 0; font-size: 12.5px; color: var(--muted); }
  .field select { width: 100%; min-width: 0; font-size: 13px; }
  .row { display: flex; justify-content: flex-end; gap: 6px; }
</style>
