<script lang="ts">
  // The gallery: every artifact as a card led by its version numeral. What
  // needs this viewer's eyes comes first; everything else follows, pinned
  // first and then the most recent. A search shows one list in that second
  // order. Each card carries its markers, who takes part and works on it, and
  // the last version this viewer saw; pin and delete once the token is known,
  // and a haiku in the footer after first paint.
  import { onDestroy, onMount } from "svelte";
  import { type Artifact, type AttentionSummary, deleteArtifact, getAttention, getToken, listArtifacts, patchArtifact } from "../api";
  import { afterPaint } from "../view/after-paint";
  import { filterArtifacts, orderArtifacts } from "../view/gallery-model";
  import GalleryCard from "./GalleryCard.svelte";
  import HaikuLine from "./HaikuLine.svelte";
  import Mark from "./Mark.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";

  let artifacts = $state<Artifact[] | null>(null);
  let error = $state<string | null>(null);
  let token = $state<string | null>(null);
  let query = $state("");
  let me = $state<string | null>(null);
  let meId = $state<string | null>(null);
  // This viewer's attention, by artifact: {} until it arrives (or on failure),
  // so the cards show in one list first and regroup once.
  let att = $state<Record<string, AttentionSummary>>({});
  // The grouping, markers and rosters (`gallery-working`), once loaded after
  // the first list paints; their feed then follows the gallery's stream, and
  // `sync` refreshes the cards an event names (`CardSync`).
  let gw = $state<typeof import("./gallery-working") | null>(null);
  let feed = $state<import("./working-feed.svelte").WorkingFeed | null>(null);
  let sync: import("./card-sync").CardSync | null = null;
  const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
  // Each list seeds the working feed; the first one, once rendered, starts its stream.
  let cancelStart: (() => void) | null = null;
  let destroyed = false;
  // A list fetched before a `working` event does not undo it (`WorkingFeed.seed`).
  const refresh = () => { const since = feed?.events; void getAttention().then(t => { if (t && !sync) att = t; }); return listArtifacts().then(a => {
    error = null; artifacts = a; feed?.seed(a, since);
    cancelStart ??= afterPaint(() => {
      void import("./gallery-working").then(m => {
        if (destroyed) return;
        const f = new m.WorkingFeed();
        f.seed(artifacts ?? []);
        sync = new m.CardSync({ list: () => artifacts, att: () => att, setList: l => { artifacts = l; }, setAtt: t => { att = t; } }, f);
        sync.start();
        feed = f; gw = m;
      }, () => {});
    });
  }, e => { error = describe(e); }); };
  const act = (id: string, op: () => Promise<unknown>) => op().then(() => sync ? sync.one(id) : refresh(), e => { error = describe(e); });
  const shown = $derived(artifacts && orderArtifacts(filterArtifacts(artifacts, query)));
  // Grouped without a query once the grouping has loaded; until then, and
  // with a query, one list.
  const g = $derived(shown && gw && !query.trim() ? gw.groups(shown, att) : null);
  onMount(() => { void refresh(); void fetch("/api/viewers/me").then(r => r.json()).then(b => { me = b?.viewer?.display_name ?? null; meId = b?.viewer?.public_id ?? null; }, () => {}); void getToken().then(t => { token = t; }); });
  onDestroy(() => { destroyed = true; cancelStart?.(); sync?.stop(); });
</script>

<header class="gbar">
  <Mark playful /><h1>Clax</h1><span class="sub hide-sm">local artifacts{#if me}{` · seen as ${me}`}{/if}</span>
  <input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" value={query} oninput={e => { query = e.currentTarget.value; }} />
  <ThemeSwitch />
</header>
<main class="gal">
  {#if error}<p class="empty">Could not load artifacts: {error}</p>{/if}
  {#if artifacts && artifacts.length === 0}
    <div class="empty-gallery"><Mark size="hero" apart /><p>When an agent publishes a page, it lands here.</p><p class="muted">Or publish one yourself with <code>clax publish index.html</code>.</p></div>
  {/if}
  {#if artifacts && artifacts.length > 0 && shown && shown.length === 0}<p class="empty">No artifacts match your search.</p>{/if}
  {#snippet card(a: Artifact)}
    {@const working = feed?.byId[a.id] ?? []}
    {@const mk = gw ? gw.cardMarkers(a, att[a.id], working) : []}
    {#snippet markerRow()}{#if gw}<gw.Markers list={mk} />{/if}{/snippet}
    <GalleryCard {a} {token} markers={mk.length ? markerRow : undefined} onPin={() => token && act(a.id, () => patchArtifact(a.id, { pinned: !a.pinned }, token!))}
      onDelete={() => { if (token && confirm(`Delete "${a.title}"? This removes every version.`)) void act(a.id, () => deleteArtifact(a.id, token!)); }}>
      {#snippet footer()}
        {#if gw}
          <gw.Roster people={a.participants?.people ?? []} agents={a.participants?.agents ?? []} {working} me={meId} max={3} small />
          <gw.Seen att={att[a.id]} />
        {/if}
      {/snippet}
    </GalleryCard>
  {/snippet}
  {#if gw && g?.needs.length}<gw.NeedsGroup list={g.needs} {card} />{/if}
  {#if shown && shown.length > 0}
    <section class="grp rest">
      {#if !query.trim()}<h2><span class="sw" aria-hidden="true"></span>{g?.needs.length ? "Everything else" : "Artifacts"}<small>pinned first, then most recent</small></h2>{/if}
      <div class="cards">{#each g?.rest ?? shown as a (a.id)}{@render card(a)}{/each}</div>
    </section>
  {/if}
  {#if artifacts}<HaikuLine footer />{/if}
</main>
