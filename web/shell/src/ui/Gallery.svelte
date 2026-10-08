<script lang="ts">
  // The gallery: every artifact as a card led by its version numeral. What
  // needs this viewer's eyes comes first; everything else follows, pinned
  // first and then the most recent. A search shows one list in that second
  // order. Each card carries its markers, who takes part and works on it, and
  // the last version this viewer saw; pin and delete once the token is known,
  // and a haiku in the footer after first paint. The question module (`q`)
  // loads after the list paints: for the owner, the inbox's unread summary
  // above everything and Inbox in the header. At `/inbox` the page is the
  // inbox instead, and `q` loads at once. `q` loads only in the owner's
  // browsers (those the token is served to); anyone else at `/inbox` is told
  // the inbox is the owner's, and nothing of it is fetched.
  import { onDestroy, onMount } from "svelte";
  import { ApiError, type Artifact, type AttentionSummary, deleteArtifact, getAttention, getToken, listArtifacts, patchArtifact } from "../api";
  import { REQUEST_STUCK, connTrouble } from "../conn-notice";
  import { Lifecycle, retrying } from "../lifecycle";
  import { afterPaint } from "../view/after-paint";
  import { filterArtifacts, orderArtifacts } from "../view/gallery-model";
  import GalleryCard from "./GalleryCard.svelte";
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
  const inbox = location.pathname === "/inbox";
  let Q = $state<typeof import("../q") | null>(null);
  // Whether this is the owner's inbox: null until known.
  let owner = $state<boolean | null>(null);
  const lazy = () => import("./gallery-working");
  // `q` comes through the lazy module, which also hands it the page's stream.
  const loadQ = (m: typeof import("./gallery-working")) => m.ownerBrowser().then(o => o ? m.q() : null).then(async q => {
    if (destroyed) return;
    if (!q) { owner = false; return; }
    Q = q;
    q.start(m.pageStream());
    owner = true;
  }, () => {});
  const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
  // Each list seeds the working feed; the first one, once rendered, starts its stream.
  let cancelStart: (() => void) | null = null;
  let destroyed = false;
  // A list fetched before a `working` event does not undo it (`WorkingFeed.seed`).
  // Owns the first load's retries; the stream and timers are `sync`'s.
  const life = new Lifecycle();
  const load = (first: boolean) => first
    ? retrying(life, signal => listArtifacts(undefined, { signal }), { retry: e => !(e instanceof ApiError), trouble: on => connTrouble("gallery", on, REQUEST_STUCK) })
    : listArtifacts();
  const refresh = (first = false) => { const since = feed?.events; void getAttention().then(t => { if (t && !sync) att = t; }); return load(first).then(a => {
    error = null; artifacts = a; feed?.seed(a, since);
    cancelStart ??= afterPaint(() => {
      void lazy().then(m => {
        if (destroyed) return;
        const f = new m.WorkingFeed();
        f.seed(artifacts ?? []);
        sync = new m.CardSync({ list: () => artifacts, att: () => att, setList: l => { artifacts = l; }, setAtt: t => { att = t; } }, f);
        sync.start();
        feed = f; gw = m;
        void loadQ(m);
      }, () => {});
    });
  }, e => { if (!life.disposed) error = describe(e); }); };
  const act = (id: string, op: () => Promise<unknown>) => op().then(() => sync ? sync.one(id) : refresh(), e => { error = describe(e); });
  const shown = $derived(artifacts && orderArtifacts(filterArtifacts(artifacts, query)));
  // Grouped without a query once the grouping has loaded; until then, and
  // with a query, one list.
  const g = $derived(shown && gw && !query.trim() ? gw.groups(shown, att) : null);
  onMount(() => { if (inbox) void lazy().then(loadQ, () => {}); else void refresh(true); void getToken().then(t => { token = t; return fetch("/api/viewers/me"); }).then(r => r.json()).then(b => { me = b?.viewer?.display_name ?? null; meId = b?.viewer?.public_id ?? null; }, () => {}); });
  onDestroy(() => { destroyed = true; life.dispose(); cancelStart?.(); sync?.stop(); Q?.stop(); });
</script>

<header class="gbar">
  <Mark playful /><h1>Clax</h1><span class="sub hide-sm">local artifacts{#if me}{` · seen as ${me}`}{/if}</span>
  {#if !inbox}<input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" value={query} oninput={e => { query = e.currentTarget.value; }} />{/if}
  {#if Q}<Q.InboxLink current={inbox} />{/if}
  <ThemeSwitch />
</header>
<main class="gal">
  {#if inbox}{#if owner && Q}<Q.InboxPage />{:else if owner === false}<p class="empty">The inbox is its owner's: open it at http://localhost:{location.port}/inbox on the computer Clax runs on.</p>{/if}{:else}
  {#if Q}<Q.GallerySummary />{/if}
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
  {#if artifacts && gw}<gw.HaikuLine footer />{/if}
  {/if}
</main>
