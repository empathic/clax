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
  // person's (`GET /api/viewers/me` says so), so a person's agents' questions
  // wait in Clax and are answered there; anyone else at `/inbox` is told
  // the browser has no inbox, and nothing more of it is fetched. At `/people` the
  // page is People (its own chunk, through the lazy module, with the page's
  // stream), and no artifacts are fetched. **People** is in the header for
  // the owner and for a connected person, once the lazy module has loaded.
  // Every one of these pages ends with a link to Clax's source on GitHub,
  // and, on a hosted Clax that shows Terms of Service and a Privacy Policy,
  // links to them; a person who has not agreed to them is asked to
  // (hosted/legal.ts). The mark in the header links back to the gallery.
  // Above the cards, the tag bar lists the tags in use: pressing tags
  // filters the gallery to the artifacts carrying every one pressed, as
  // does a card's tag (adding it), and the filter is the address's `?tag=`
  // (each change a history entry, so back and forward walk the filters).
  // A tag filter, like a search (which matches tag names too), shows one
  // list. The bar links to the tags at `/tags`, the Tags page (its own
  // chunk, through the lazy module, like People), where no artifacts are
  // fetched.
  import { onDestroy, onMount } from "svelte";
  import { ApiError, type Artifact, type AttentionSummary, type TagRef, deleteArtifact, getAttention, getToken, listArtifacts, patchArtifact } from "../api";
  import { REQUEST_STUCK, connTrouble } from "../conn-notice";
  import { Lifecycle, retrying } from "../lifecycle";
  import { afterPaint } from "../view/after-paint";
  import { filterArtifacts, orderArtifacts } from "../view/gallery-model";
  import { barTags, canonical, carriesAll, holds, refsForUrl, same, tagsFromUrl, tagsInUse, toggle, withTags } from "../view/tags-model";
  import GalleryCard from "./GalleryCard.svelte";
  import Mark from "./Mark.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";

  let artifacts = $state<Artifact[] | null>(null);
  let error = $state<string | null>(null);
  let token = $state<string | null>(null);
  // A hosted daemon's gallery names its site rather than "local artifacts":
  // the owner's browser holds its shell token (`cxo_`), anyone else asks
  // `GET /api/hosted/me` (404 on a local daemon); both ask it for the
  // documents to link (`legal`).
  let site = $state<string | null>(null);
  let legal = $state<{ terms_url: string; privacy_url: string } | null>(null);
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
  const tagsAt = location.pathname === "/tags";
  // The tag filter: the artifacts shown carry every one. It holds tag IDs
  // once the tags are known (`canonical`), and the address (`?tag=`) names
  // each tag by its name, so a rename follows in place and a deleted tag
  // drops out. While it filters, the whole list (`all`) is fetched, again
  // at each `tags` event, to tell a deleted tag from one no card carries.
  let sel = $state(tagsFromUrl(location.search));
  let all = $state<TagRef[] | null>(null);
  const used = $derived(tagsInUse(artifacts ?? []));
  const known = $derived([...used, ...(all ?? [])]);
  const url = (next: string[]) => `${location.pathname}${withTags(location.search, refsForUrl(next, known))}${location.hash}`;
  const setSel = (next: string[]) => { sel = next; history.pushState(history.state, "", url(next)); };
  const onTag = (t: TagRef) => { if (!holds(sel, t)) setSel([...sel, t.id]); document.querySelector(".tagbar")?.scrollIntoView?.({ block: "nearest" }); };
  const inUse = $derived(barTags(used, sel, all));
  const loadAll = () => { if (sel.length) void fetch("/api/tags").then(r => (r.ok ? r.json() : null)).then((b: { tags: TagRef[] } | null) => { if (b && !destroyed) all = b.tags; }, () => {}); };
  $effect(() => { if (sel.length && all === null && artifacts) loadAll(); });
  // The filter in its canonical form, and the address naming it, in place.
  $effect(() => {
    if (!artifacts) return;
    const next = canonical(sel, known, all);
    if (!same(next, sel)) sel = next;
    const u = url(next);
    if (u !== `${location.pathname}${location.search}${location.hash}`) history.replaceState(history.state, "", u);
  });
  let unPeople: (() => void) | undefined;
  let Q = $state<typeof import("../q") | null>(null);
  let Page = $state<typeof import("../q/InboxPage.svelte").default | null>(null);
  let Summary = $state<typeof import("../q/GallerySummary.svelte").default | null>(null);
  // Whether this browser has an inbox (the owner's or a connected person's): null until known.
  let owner = $state<boolean | null>(null);
  const lazy = () => import("./gallery-working");
  // `q` comes through the lazy module, which also hands it the page's stream.
  // A gallery destroyed meanwhile asks no more and imports nothing: a late
  // import would load the question module after the page (or a test) moved on.
  // `/inbox` loads its page's own chunk beside the module, and `/` the
  // unread summary's.
  // The lazy module, once loaded (on every path): People in the header comes from it.
  let lm = $state<typeof import("./gallery-working") | null>(null);
  const loadQ = (m: typeof import("./gallery-working")) => { if (!destroyed) lm = m; return m.inboxBrowser(mine, life.signal).then(o => (o && !destroyed ? Promise.all([m.q(), inbox ? m.inboxPage() : null, inbox || people ? null : m.summary()]) : null)).then(async r => {
    if (destroyed) return;
    if (!r) { owner = false; return; }
    Q = r[0];
    Q.start(m.pageStream());
    owner = true;
    Page = r[1]?.default ?? null;
    Summary = r[2]?.default ?? null;
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
        sync = new m.CardSync({ list: () => artifacts, att: () => att, setList: l => { artifacts = l; }, setAtt: t => { att = t; }, tags: loadAll }, f);
        sync.start();
        feed = f; gw = m;
        void loadQ(m);
      }, () => {});
    });
  }, e => { if (!life.disposed) error = describe(e); }); };
  const act = (id: string, op: () => Promise<unknown>) => op().then(() => sync ? sync.one(id) : refresh(), e => { error = describe(e); });
  const shown = $derived(artifacts && orderArtifacts(filterArtifacts(artifacts, query).filter(a => carriesAll(a, sel))));
  // Grouped without a query or a tag filter once the grouping has loaded;
  // until then, and with either, one list.
  const flat = $derived(!!query.trim() || sel.length > 0);
  const g = $derived(shown && gw && !flat ? gw.groups(shown, att) : null);
  // This browser's viewer, asked once (after the token: on this machine that makes it the owner's).
  let mine: Promise<import("../people/PeopleLink.svelte").Viewer>;
  onMount(() => { mine = getToken().then(t => { token = t; if (t?.startsWith("cxo_")) site = location.host; if (!t || t.startsWith("cxo_")) void fetch("/api/hosted/me").then(r => r.ok && r.json()).then((h: import("../hosted/api").HostedMe | false) => { if (h && !destroyed) { site = location.host; legal = h.legal ?? null; if (legal) void import("../hosted/legal").then(m => m.ask(h.legal!)); } }, () => {}); return fetch("/api/viewers/me"); }).then(r => r.json()).then(b => (viewer = b?.viewer), () => null); if (inbox) void lazy().then(loadQ, () => {}); else if (people || tagsAt) void lazy().then(m => { void loadQ(m); if (!destroyed) unPeople = (people ? m.peoplePage : m.tagsPage)(document.querySelector("main.gal")!); }, () => {}); else void refresh(true); });
  onDestroy(() => { destroyed = true; unPeople?.(); life.dispose(); cancelStart?.(); sync?.stop(); Q?.stop(); });
  // Back and forward walk the tag filters.
  const popped = () => { sel = tagsFromUrl(location.search); };
</script>

<svelte:window onpopstate={popped} />

<header class="gbar">
  <a href="/" class="home" aria-label="Gallery"><Mark /></a><h1>Clax</h1><span class="sub hide-sm">{site ?? "local artifacts"}{#if me}{` · seen as ${me}`}{/if}</span>
  {#if !inbox && !people && !tagsAt}<input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" value={query} oninput={e => { query = e.currentTarget.value; }} />{/if}
  {#if Q}<Q.InboxLink current={inbox} />{/if}
  {#if lm}<lm.Links current={people} owner={!!token} {viewer} />{/if}
  <ThemeSwitch />
</header>
<main class="gal">
  {#if inbox}{#if owner && Page}<Page stream={lm?.pageStream()} />{:else if owner === false}<p class="empty">This browser has no inbox: open the owner's at http://localhost:{location.port}/inbox on the computer Clax runs on.</p>{/if}{:else if !people && !tagsAt}
  {#if Summary}<Summary />{/if}
  {#if error}<p class="empty">Could not load artifacts: {error}</p>{/if}
  {#if artifacts && artifacts.length === 0}
    <div class="empty-gallery"><Mark size="hero" apart /><p>When an agent publishes a page, it lands here.</p><p class="muted">Or publish one yourself with <code>clax publish index.html</code>.</p></div>
  {/if}
  {#if artifacts?.length && inUse.length}
    <div class="tagbar" role="group" aria-label="Filter by tags">Tags
      {#each inUse as t (t.id)}<button type="button" class="tag" data-c={t.colour} aria-pressed={holds(sel, t)} aria-label={`Filter by tag ${t.name}`} onclick={() => setSel(toggle(sel, t))}>{t.name}</button>{/each}
      {#if sel.length}<button type="button" class="ghost tb-x" onclick={() => setSel([])}>Clear</button>{/if}
      <a class="tb-m" href="/tags">{token ? "Manage tags" : "All tags"}</a>
    </div>
  {/if}
  {#if artifacts && artifacts.length > 0 && shown && shown.length === 0}<p class="empty">{query.trim() ? "No artifacts match your search." : "No artifacts carry every tag chosen."}</p>{/if}
  {#snippet card(a: Artifact)}
    {@const working = feed?.byId[a.id] ?? []}
    {@const mk = gw ? gw.cardMarkers(a, att[a.id], working) : []}
    {#snippet markerRow()}{#if gw}<gw.Markers list={mk} />{/if}{/snippet}
    <GalleryCard {a} {token} {onTag} markers={mk.length ? markerRow : undefined} onPin={() => token && act(a.id, () => patchArtifact(a.id, { pinned: !a.pinned }, token!))}
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
      {#if !flat}<h2><span class="sw" aria-hidden="true"></span>{g?.needs.length ? "Everything else" : "Artifacts"}<small>pinned first, then most recent</small></h2>{/if}
      <div class="cards">{#each g?.rest ?? shown as a (a.id)}{@render card(a)}{/each}</div>
    </section>
  {/if}
  {#if artifacts && gw}<gw.HaikuLine footer />{/if}
  {/if}
</main>
<nav class="glegal" aria-label="About Clax">{#if legal}<a href={legal.terms_url}>Terms of Service</a>{" · "}<a href={legal.privacy_url}>Privacy Policy</a>{" · "}{/if}<a href="https://github.com/empathic/clax" target="_blank" rel="noopener">Source on GitHub</a></nav>
