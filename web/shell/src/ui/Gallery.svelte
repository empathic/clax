<script lang="ts">
  // The gallery: every artifact as a card led by its version numeral. What
  // needs this viewer's eyes comes first; everything else follows, pinned
  // first and then the most recent. A search shows one list in that second
  // order. Each card carries its markers, who takes part and works on it, and
  // the last version this viewer saw; pin and delete once the token is known,
  // and a haiku in the footer after first paint. The question module (`q`)
  // loads after the list paints: for the owner, the inbox's unread summary
  // above everything and Inbox in the header. At `/inbox` the page is the
  // inbox instead, and `q` loads at once. `q` loads only in a browser with
  // an inbox: the owner's (the token is served to it) or a connected
  // person's (`GET /api/inbox` answers it), so a person's agents' questions
  // wait in Clax and are answered there; anyone else at `/inbox` is told
  // the browser has no inbox, and nothing more of it is fetched. At `/people` the
  // page is People (its own chunk, through the lazy module, with the page's
  // stream), and no artifacts are fetched. **People** is in the header for
  // the owner and for a connected person, once the lazy module has loaded.
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
  // This browser's viewer (`GET /api/viewers/me`): its name, public ID, and
  // whether it is a connected person's.
  let viewer = $state<import("../people/PeopleLink.svelte").Viewer>(null);
  const me = $derived(viewer?.display_name);
  const meId = $derived(viewer?.public_id ?? null);
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
  const people = location.pathname === "/people";
  let unPeople: (() => void) | undefined;
  let Q = $state<typeof import("../q") | null>(null);
  let Page = $state<typeof import("../q/InboxPage.svelte").default | null>(null);
  // Whether this browser has an inbox (the owner's or a connected person's): null until known.
  let owner = $state<boolean | null>(null);
  const lazy = () => import("./gallery-working");
  // `q` comes through the lazy module, which also hands it the page's stream.
  // A gallery destroyed meanwhile asks no more and imports nothing: a late
  // import would load the question module after the page (or a test) moved on.
  // `/inbox` loads its page's own chunk beside the module.
  // The lazy module, once loaded (on every path): People in the header comes from it.
  let lm = $state<typeof import("./gallery-working") | null>(null);
  const loadQ = (m: typeof import("./gallery-working")) => { if (!destroyed) lm = m; return m.inboxBrowser(life.signal).then(o => (o && !destroyed ? Promise.all([m.q(), inbox ? m.inboxPage() : null]) : null)).then(async r => {
    if (destroyed) return;
    if (!r) { owner = false; return; }
    Q = r[0];
    Q.start(m.pageStream());
    owner = true;
    Page = r[1]?.default ?? null;
  }, () => {}); };
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
  onMount(() => { if (inbox) void lazy().then(loadQ, () => {}); else if (people) void lazy().then(m => { void loadQ(m); if (!destroyed) unPeople = m.peoplePage(document.querySelector("main.gal")!); }, () => {}); else void refresh(true); void getToken().then(t => { token = t; return fetch("/api/viewers/me"); }).then(r => r.json()).then(b => { viewer = b?.viewer; }, () => {}); });
  onDestroy(() => { destroyed = true; unPeople?.(); life.dispose(); cancelStart?.(); sync?.stop(); Q?.stop(); });
</script>

<header class="gbar">
  <Mark playful /><h1>Clax</h1><span class="sub hide-sm">local artifacts{#if me}{` · seen as ${me}`}{/if}</span>
  {#if !inbox && !people}<input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" value={query} oninput={e => { query = e.currentTarget.value; }} />{/if}
  {#if Q}<Q.InboxLink current={inbox} />{/if}
  {#if lm}<lm.PeopleLink current={people} owner={!!token} {viewer} />{/if}
  <ThemeSwitch />
</header>
<main class="gal">
  {#if inbox}{#if owner && Page}<Page />{:else if owner === false}<p class="empty">This browser has no inbox. The owner's is at http://localhost:{location.port}/inbox on the computer Clax runs on; a connected person's is in the browser <code>clax connect</code> opened.</p>{/if}{:else if !people}
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
