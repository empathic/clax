<script lang="ts">
  // The gallery's sites (spec 2026-10-05-chrome-overlay-design §7.2, owner
  // decision 2026-10-06): one entry per site Clax has live pages of, a
  // joined site's named after its most recently used address and listing
  // the others, with its pages and comments. Each entry's menu joins it to
  // another site ("Same app as…", repeated while the daemon says threads
  // remain to merge) or splits an address off. Loaded after the gallery's
  // first paint (`gallery-working`), and only when it lists a live page;
  // the menu needs the token.
  import { onMount } from "svelte";
  import { getToken } from "../api";

  type Origin = { origin: string; last_used_at: string | null };
  type Site = { site: { key: string; name: string; joined: boolean; origins: Origin[] }; pages: number; threads: number };
  let sites = $state<Site[] | null>(null);
  let token = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let note = $state<{ text: string; bad: boolean } | null>(null);
  let picks = $state<Record<string, string>>({});
  /** The most batches one join sends (200 threads each). */
  const MAX_BATCHES = 1000;
  const host = (o: string) => o.replace(/^\w+:\/\//, "");
  const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

  async function load(): Promise<void> {
    const r = await fetch("/api/live/sites").catch(() => null);
    if (r?.ok) sites = ((await r.json()) as { sites: Site[] }).sites;
  }
  async function post(path: string, body: unknown): Promise<{ remaining?: number; moved?: string[] }> {
    const r = await fetch(path, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify(body) });
    const v = await r.json().catch(() => null);
    if (!r.ok) throw new Error(v?.error?.message ?? `${r.status} ${r.statusText}`);
    return v ?? {};
  }
  async function act(what: string, f: () => Promise<string>): Promise<void> {
    if (busy) return;
    busy = what;
    note = null;
    try {
      note = { text: await f(), bad: false };
    } catch (e) {
      note = { text: e instanceof Error ? e.message : String(e), bad: true };
    } finally {
      busy = null;
      await load();
    }
  }
  const join = (s: Site, other: Site) => act(`Joining ${host(s.site.name)} and ${host(other.site.name)}`, async () => {
    let moved = 0;
    for (let i = 0; i < MAX_BATCHES; i++) {
      const r = await post("/api/live/sites/join", { origin: s.site.key, with: other.site.key });
      moved += r.moved?.length ?? 0;
      if (!r.remaining) return `Joined: ${host(s.site.name)} and ${host(other.site.name)} are one site${moved ? `, ${plural(moved, "thread")} merged` : ""}.`;
      if (!r.moved?.length) break;
    }
    throw new Error("Stopped before the join finished. Try again to finish it.");
  });
  const split = (o: string) => act(`Splitting ${host(o)} off`, async () => {
    await post("/api/live/sites/split", { origin: o });
    return `${host(o)} is a site of its own again. What the site has kept stays with it.`;
  });
  onMount(() => { void getToken().then(t => { token = t; }); void load(); });
</script>

{#if sites?.length}
  <!-- Styled with the theme's classes and inline styles: a lazy part of the
       shell carries no stylesheet (the shell's CSS comes from its HTML). -->
  <section class="grp" aria-label="Sites">
    <h2><span class="sw" aria-hidden="true" style="border-radius:50%;width:10px;height:10px;background:var(--border-strong)"></span>Sites<small>live pages by address</small></h2>
    <ul style="list-style:none;margin:10px 0 0;padding:0;display:grid;gap:6px">
      {#each sites as s (s.site.key)}
        {@const others = s.site.origins.filter(o => o.origin !== s.site.name)}
        <li class="site" style="display:flex;align-items:flex-start;gap:8px;padding:10px 12px;border:1px solid var(--border);border-radius:var(--radius-sm);background:var(--card)">
          <div style="flex:1;min-width:0;display:grid;gap:2px;overflow-wrap:anywhere">
            <strong title={s.site.name} style="font:600 13.5px/1.4 var(--mono)">{host(s.site.name)}</strong>
            {#if others.length}<span class="also muted small">also {others.map(o => host(o.origin)).join(", ")}</span>{/if}
            <span class="muted small">{plural(s.pages, "page")} · {plural(s.threads, "comment thread")}</span>
          </div>
          {#if token}
            <details style="position:relative">
              <summary aria-label={`Site menu for ${host(s.site.name)}`} style="cursor:pointer;list-style:none;padding:0 8px" class="muted">⋯</summary>
              <div style="position:absolute;right:0;z-index:2;display:grid;gap:6px;width:min(260px,calc(100vw - 32px));padding:10px;border:1px solid var(--border-strong);border-radius:var(--radius-sm);background:var(--card)">
                <label class="muted small" style="display:grid;gap:4px"><span>Same app as…</span>
                  <select aria-label={`Same app as, for ${host(s.site.name)}`} onchange={e => { picks[s.site.key] = e.currentTarget.value; }}>
                    <option value="">Choose a site</option>
                    {#each sites.filter(o => o.site.key !== s.site.key) as o (o.site.key)}<option value={o.site.key}>{host(o.site.name)}</option>{/each}
                  </select>
                </label>
                <button type="button" disabled={!picks[s.site.key] || !!busy}
                  onclick={() => { const o = sites?.find(x => x.site.key === picks[s.site.key]); if (o) void join(s, o); }}>Join</button>
                {#if s.site.joined}
                  {#each s.site.origins as o (o.origin)}
                    <button type="button" class="ghost" disabled={!!busy} onclick={() => split(o.origin)}>Split {host(o.origin)} off</button>
                  {/each}
                {/if}
              </div>
            </details>
          {/if}
        </li>
      {/each}
    </ul>
    {#if busy}<p class="rule" role="status">{busy}…</p>{:else if note?.bad}<p class="rule error" role="status">{note.text}</p>{:else if note}<p class="rule" role="status">{note.text}</p>{/if}
  </section>
{/if}
