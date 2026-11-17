<script lang="ts">
  // The gallery: every artifact as a card led by its version numeral. What
  // needs this viewer's eyes comes first; everything else follows, pinned
  // first and then the most recent. A search shows one list in that second
  // order. Each card carries its markers, who takes part and works on it, and
  // the last version this viewer saw; pin and delete for the owner; a haiku
  // in the footer once the lazy module is in. For a browser with an inbox
  // (the owner's, or a connected person's, `GET /api/viewers/me` saying so),
  // the inbox's unread summary leads, and Inbox is in the header; so a
  // person's agents' questions wait in Clax and are answered there; anyone
  // else at `/inbox` is told the browser has no inbox, and nothing more of
  // it is fetched.
  //
  // The page loads in its final shape (view/gallery-start.ts): the entry's
  // static frame (view/gallery-frame.ts) is on screen before any script,
  // and this component takes its place showing the same frame. The header
  // renders its links and subtitle once, when the answers that decide them
  // are in; the content (the summary, the tag bar, the groups and their
  // cards with their markers and rosters) renders once, in place of the
  // skeleton, when everything that lays it out is in. Those answers come
  // from the daemon's bootstrap block (`boot`, gallery-boot.ts) where it
  // has them, so a page with the whole block renders its final layout at
  // mount, and the rest are asked all at once. The live refreshes
  // (`CardSync`), the working lists and the question module's feeds start
  // after that render, through the lazy module (`gallery-working`), and a
  // refresh that finds nothing new changes nothing on screen. The skeleton
  // takes the shape of the content before the first paint (the inline
  // `clax-shape` script, view/gallery-frame.ts): once the content is on
  // screen, its shape is kept for the next load (view/gallery-shape.ts).
  // While the skeleton shows, `main` is busy and says so politely; a chunk
  // that does not load ends it with a message and a way to reload. ClaxBot's
  // usage banner, from the header's answers, leads the content from its
  // first render.
  //
  // At `/inbox` the page is the inbox, at `/settings` and
  // `/settings/<screen>` Settings (its own chunk, through the lazy module,
  // with the page's stream), at `/queues` the report queues (with a queue's
  // view at `/queues/<qid>` and a report's at `/queues/<qid>/<tid>`), at
  // `/tags` the Tags page; no artifacts are fetched on them, and each shows
  // the skeleton until its chunk and first data are in. The pages that
  // moved into Settings (`/people`, `/admin`, `/me`, `/audit`) go to their
  // screens there, by the lazy module too (`settings/moved.ts`). The
  // header's links (GalleryLinks) are Inbox, Queues, the feedback menu
  // (`feedback/`), for a person who may report problems with Clax, and the
  // Settings gear, before the search box and the theme switch.
  // Every one of these pages ends with a link to Clax's source on GitHub,
  // and, on a hosted Clax that shows Terms of Service and a Privacy Policy,
  // links to them; a person who has not agreed to them is asked to
  // (hosted/legal.ts). The mark in the header links back to the gallery.
  // Above the cards, the tag bar lists the tags in use: pressing tags
  // filters the gallery to the artifacts carrying every one pressed, as
  // does a card's tag (adding it), and the filter is the address's `?tag=`
  // (each change a history entry, so back and forward walk the filters).
  // A tag filter, like a search (which matches tag names too), shows one
  // list. The bar links to the tags at `/tags`. The New artifact tile
  // (`NewTile`) leads the first section's grid (searched or filtered too),
  // and the empty gallery. Signing out of a hosted Clax is in Settings.
  import { type Component, onDestroy, onMount, tick } from "svelte";
  import { type Artifact, type AttentionSummary, type TagRef, deleteArtifact, getToken, listArtifacts, patchArtifact, tokenUnsure } from "../api";
  import type { MeBot } from "../bot/api";
  import { banner } from "../bot/meter";
  import { reasonText, unreached } from "../trouble";
  import { loadWait } from "../reach";
  import { REQUEST_STUCK, connTrouble } from "../conn-notice";
  import { readConfig } from "../feedback/boot";
  import { configOnce } from "../feedback/config";
  import { feedbackMenu } from "../feedback/slot.svelte";
  import { Lifecycle, retrying } from "../lifecycle";
  import { ownerProof } from "../owner-proof";
  import { afterPaint } from "../view/after-paint";
  import { groups, markers } from "../view/attention-model";
  import type { GalleryBoot } from "../view/gallery-boot";
  import { REST_AFTER_NEEDS, REST_HEADING, REST_NOTE, SKELETON_HTML } from "../view/gallery-frame";
  import { keepShape, measureShape } from "../view/gallery-shape";
  import { filterArtifacts, orderArtifacts } from "../view/gallery-model";
  import { type Answer, type Content, type Header, type Io, type TokenAnswer, contentFromBlock, gatherContent, gatherHeader, header as decide, headerFromBlock, topOf } from "../view/gallery-start";
  import { barTags, canonical, carriesAll, holds, refsForUrl, same, tagsFromUrl, tagsInUse, toggle, withTags } from "../view/tags-model";
  import { agentNames, chips } from "../view/working-model";
  import GalleryCard from "./GalleryCard.svelte";
  import GalleryLinks from "./GalleryLinks.svelte";
  import Mark from "./Mark.svelte";
  import Markers from "./Markers.svelte";
  import NeedsGroup from "./NeedsGroup.svelte";
  import NewTile from "./NewTile.svelte";
  import Roster from "./Roster.svelte";
  import Seen from "./Seen.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";

  type Lazy = typeof import("./gallery-working");
  const p: { boot?: GalleryBoot | null } = $props();
  // svelte-ignore state_referenced_locally
  const boot = p.boot ?? null;
  // The page: the inbox, a page beside the gallery (`gallery-working`
  // `page`: any other one-word path the daemon serves this entry at), or
  // the gallery's list.
  const top = topOf(location.pathname);
  const inbox = top === "/inbox";
  const sub = inbox ? undefined : /^\/([a-z]+)$/.exec(top)?.[1] as import("./gallery-working").Page | undefined;
  const listing = !inbox && !sub;

  // The header, once its answers are in (at once from a whole block).
  let hdr = $state<Header | null>(headerFromBlock(boot));
  // Whether the browser has an inbox, when the header's answers could not
  // say (the token route failing) and the owner check said later.
  let lateInbox = $state<boolean | null>(null);
  const hasInbox = $derived(hdr?.inbox ?? lateInbox);
  const me = $derived(hdr?.me?.viewer?.display_name);
  const meId = $derived(hdr?.me?.viewer?.public_id ?? null);
  const legal = $derived(hdr?.hosted?.legal ?? null);
  // Back and forward come back to where the person was on the list: the
  // browser's own restoring would scroll the skeleton, and the footer under
  // it, before the content is in, so the gallery keeps the place in its
  // history entry; the entry's inline script holds `main` tall enough to be
  // there at once (`--gback`), the gallery scrolls there at mount and lets
  // `main` go as the content lands.
  const KEPT = "clax.galleryY";
  const back = listing ? kept() : 0;
  function kept(): number {
    try { history.scrollRestoration = "manual"; } catch { /* the browser's own then */ }
    const nav = (performance.getEntriesByType?.("navigation")[0] as PerformanceNavigationTiming | undefined)?.type;
    return nav === "back_forward" ? Number((history.state as Record<string, unknown> | null)?.[KEPT] ?? 0) || 0 : 0;
  }
  const keep = () => { try { history.replaceState({ ...(history.state as object | null), [KEPT]: scrollY }, ""); } catch { /* not kept */ } };
  // The content: shown once everything that lays it out is in.
  // svelte-ignore state_referenced_locally
  const first: Content | null = listing ? contentFromBlock(boot, hdr) : null;
  let ready = $state(false);
  let artifacts = $state<Artifact[] | null>(null);
  let error = $state<string | null>(null);
  // This viewer's attention, by artifact ({} without a viewer, or on failure).
  let att = $state<Record<string, AttentionSummary>>({});
  // svelte-ignore state_referenced_locally
  if (first && hdr && !hdr.inbox && !banner(hdr.bot as unknown as MeBot)) { artifacts = first.artifacts; error = first.error; att = first.attention; ready = true; }
  let query = $state("");
  // The lazy module, its working lists' feed and the live refreshes, once the content is on screen.
  let lm = $state<Lazy | null>(null);
  let feed = $state<import("./working-feed.svelte").WorkingFeed | null>(null);
  let sync: import("./card-sync").CardSync | null = null;
  let Q = $state<typeof import("../q") | null>(null);
  let Page = $state<typeof import("../q/InboxPage.svelte").default | null>(null);
  let Summary = $state<typeof import("../q/GallerySummary.svelte").default | null>(null);
  // A chunk the page needs did not load: the skeleton gives way to a message.
  let failed = $state(false);
  // ClaxBot's usage banner (from 80 % of the week's credits), from the header's answer.
  // Its own chunk, loaded only for a banner to show, which the content's
  // first render waits for: it is never inserted above content on screen.
  const ban = $derived(hdr?.bot ? banner(hdr.bot as unknown as MeBot) : null);
  let Banner = $state<Component<{ me: MeBot }> | null>(null);
  // The banner's chunk is in, or none is needed: a page beside the gallery may show.
  let bannerIn = $state(false);
  let bannerLoad: Promise<void> | null = null;
  const loadBanner = (h: Header) => (bannerLoad ??= banner(h.bot as unknown as MeBot) ? import("../bot/UsageBanner.svelte").then(m => { Banner = m.default; }, () => {}) : Promise.resolve());
  const unread = $derived(Q ? Q.inboxFeed().unread : hdr?.summary?.unread ?? 0);
  // A page beside the gallery mounts into `pageHost`, shown in the
  // skeleton's place once it has its first data. Until then it is laid out
  // (so what it focuses takes focus) with no height, before the skeleton.
  let pageHost: HTMLElement | undefined = $state();
  let pageShown = $state(false);
  // The page is shown once it has its first data and the header (and with it the banner) is in.
  const revealed = $derived(pageShown && !!hdr && bannerIn);
  // The skeleton shows (`main` busy): until the content, the page or a failure ends it.
  const pending = $derived(!failed && (listing ? !ready : inbox ? hasInbox !== false && !revealed : !revealed));
  /** Shows the page; resolves once it is on screen. */
  const shownPage = () => { pageShown = true; return tick(); };
  let unPage: (() => void) | undefined;
  let destroyed = false;
  let cancelLive: (() => void) | undefined;
  // Owns the first load's retries; the stream and timers are `sync`'s.
  const life = new Lifecycle();
  const lazy = () => import("./gallery-working");

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

  const describe = reasonText;
<<<<<<< HEAD
  const get = (path: string): Promise<Answer<unknown>> => fetch(path).then(async r => ({ status: r.status, body: r.ok ? await r.json().catch(() => null) : null }), () => ({ status: 0, body: null }));
  const tokenP: Promise<TokenAnswer> = getToken().then(t => (t === null && tokenUnsure() ? "failed" : t));
  const io: Io = {
    get, token: () => tokenP, feedback: configOnce, proof: ownerProof() !== null,
    list: () => retrying(life, signal => listArtifacts(undefined, { signal }), { retry: unreached, trouble: on => connTrouble("gallery", on, REQUEST_STUCK) }),
  };
  /** The token for an owner's action: asked at mount, an action waits for it. */
  const withToken = (f: (t: string) => Promise<unknown>) => tokenP.then(t => (typeof t === "string" ? f(t) : Promise.reject(new Error("only the owner may do this"))));
  const reload = () => listArtifacts().then(a => { if (!destroyed) artifacts = a; });
  const act = (id: string, op: () => Promise<unknown>) => op().then(() => (sync ? sync.one(id) : reload()), e => { error = describe(e); });
||||||| parent of 664df2ce4 (fix(shell): lifecycle.ts stays free of imports, and ClaxBot's, Queues' and sites' refusals name no status)
  // Each list seeds the working feed; the first one, once rendered, starts its stream.
  let cancelStart: (() => void) | null = null;
  let destroyed = false;
  // A list fetched before a `working` event does not undo it (`WorkingFeed.seed`).
  // Owns the first load's retries; the stream and timers are `sync`'s.
  const life = new Lifecycle();
  const load = (first: boolean) => first
    ? retrying(life, signal => listArtifacts(undefined, { signal }), { retry: unreached, trouble: on => connTrouble("gallery", on, REQUEST_STUCK) })
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
=======
  // Each list seeds the working feed; the first one, once rendered, starts its stream.
  let cancelStart: (() => void) | null = null;
  let destroyed = false;
  // A list fetched before a `working` event does not undo it (`WorkingFeed.seed`).
  // Owns the first load's retries; the stream and timers are `sync`'s.
  const life = new Lifecycle();
  const load = (first: boolean) => first
    ? retrying(life, signal => listArtifacts(undefined, { signal }), { retry: unreached, wait: loadWait, trouble: on => connTrouble("gallery", on, REQUEST_STUCK) })
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
>>>>>>> 664df2ce4 (fix(shell): lifecycle.ts stays free of imports, and ClaxBot's, Queues' and sites' refusals name no status)
  const shown = $derived(artifacts && orderArtifacts(filterArtifacts(artifacts, query).filter(a => carriesAll(a, sel))));
  // Grouped without a query or a tag filter; with either, one list.
  const flat = $derived(!!query.trim() || sel.length > 0);
  const g = $derived(shown && !flat ? groups(shown, att) : null);

  // The question module, loaded once for a browser with an inbox, through
  // the lazy module (which hands it the page's stream), and seeded with
  // what the first load read, so its surfaces show that before its feeds
  // go live. A gallery destroyed meanwhile imports nothing more.
  let qLoad: Promise<typeof import("../q") | null> | null = null;
  const loadQ = (h: Header, open: Content["questions"] = null) => (qLoad ??= lazy().then(m => (destroyed ? null : m.q())).then(q => {
    if (!q || destroyed) return null;
    q.seed(h.summary, open);
    return q;
  }, () => null));
  const startQ = (q: typeof import("../q") | null) => { if (!q || destroyed) return; void lazy().then(m => { if (destroyed) return; Q = q; q.start(m.pageStream()); }); };

  /** What the header's answers start: the legal ask, ClaxBot's answer for the page's other asks (`seedBot`), the question module. */
  function headerIn(h: Header) {
    if (h.hosted?.legal) { const terms = h.hosted.legal; void import("../hosted/legal").then(m => m.ask(terms)); }
    if (h.bot) void lazy().then(m => { m.seedBot(h.bot as never); }, () => {});
    if (inbox) {
      if (h.inbox) void Promise.all([loadQ(h), lazy().then(m => m.inboxPage())]).then(([q, pg]) => { if (destroyed) return; if (!q) { failed = true; return; } Page = pg.default; startQ(q); }, () => { failed = true; });
    } else if (!listing && h.inbox) void loadQ(h).then(startQ);
    if (h.inbox === null) {
      // The token route failed: whether this browser is the owner's is asked
      // until it answers. Said later, it brings Inbox and its count into the
      // header's links (which move nothing), and at /inbox the page; on the
      // gallery's list, not the unread summary, which would push the cards
      // down under the person's eyes.
      void lazy().then(m => m.inboxBrowser(Promise.resolve(h.me?.viewer), life.signal)).then(o => {
        if (destroyed) return;
        lateInbox = o;
        if (!o) return;
        const now = { ...h, inbox: true };
        if (inbox) headerIn(now);
        else void loadQ(now).then(startQ);
      }, () => { if (inbox && !destroyed) failed = true; });
    }
  }

  /** The live refreshes, the working lists and the haiku, once the content is on screen. */
  function live() {
    if (destroyed) return;
    void lazy().then(m => {
      if (destroyed) return;
      const f = new m.WorkingFeed();
      f.seed(artifacts ?? []);
      sync = new m.CardSync({ list: () => artifacts, att: () => att, setList: l => { artifacts = l; }, setAtt: t => { att = t; }, tags: loadAll }, f);
      sync.start();
      feed = f; lm = m;
    }, () => {});
  }

  // A Settings screen found this browser no longer a member's: the header asks again.
  const recheck = () => {
    void Promise.all([get("/api/viewers/me"), get("/api/hosted/me"), tokenP]).then(([viewer, hosted, t]) => {
      if (destroyed || !hdr) return;
      const was = hdr;
      hdr = { ...decide({ viewer: viewer as Answer<never>, hosted: hosted as Answer<never>, token: t }, null), feedback: was.feedback, bot: was.bot, summary: was.summary };
    });
  };

  /** Keeps the page's shape for the next load's skeleton (view/gallery-shape.ts). */
  const keepNow = () => {
    if (failed || (listing ? !ready || error || query.trim() || sel.length : !revealed)) return;
    keepShape(top, measureShape(document, listing && artifacts ? { need: g?.needs.length ?? 0, rest: g?.rest.length ?? 0, tile: hdr?.me?.may_create === true, empty: artifacts.length === 0 } : undefined));
  };
  // Measured once its first render is painted, laid out as the person sees it.
  let cancelKeep: (() => void) | undefined;
  $effect(() => { if ((listing ? ready : revealed) && !cancelKeep) cancelKeep = afterPaint(keepNow); });

  onMount(() => {
    addEventListener("clax:people-gone", recheck);
    const headerP = hdr ? Promise.resolve(hdr) : gatherHeader(boot, io, tokenP);
    // The links' remembered width held their slot until now (the inline clax-shape script).
    void headerP.then(h => { if (destroyed) return; hdr = h; void loadBanner(h).then(() => { bannerIn = true; }); document.documentElement.style.removeProperty("--sk-links"); headerIn(h); });
    if (sub) void lazy().then(m => { if (!destroyed && pageHost) unPage = m.page(sub, pageHost, shownPage, () => { failed = true; }); }, () => { failed = true; });
    if (!listing) { void lazy().then(m => { if (!destroyed) lm = m; }, () => {}); return; }
    addEventListener("pagehide", keep); addEventListener("click", keep, true);
    if (back) scrollTo(0, back);
    // The live refreshes follow a list that loaded; a refused one stays as it was said.
    if (ready) { document.documentElement.style.removeProperty("--gback"); if (artifacts) cancelLive = afterPaint(live); return; }
    const contentP = first ? Promise.resolve(first) : gatherContent(boot, io, headerP);
    void Promise.all([contentP, headerP]).then(async ([c, h]) => {
      let S: typeof Summary = null;
      await loadBanner(h);
      if (h.inbox) {
        const [q, s] = await Promise.all([loadQ(h, c.questions), lazy().then(m => m.summary()).catch(() => null)]);
        if (destroyed) return;
        S = s?.default ?? null;
        startQ(q);
      }
      if (destroyed) return;
      artifacts = c.artifacts; error = c.error; att = c.attention; Summary = S; ready = true;
      if (back) void tick().then(() => { document.documentElement.style.removeProperty("--gback"); if (!destroyed && scrollY !== back) scrollTo(0, back); });
      if (artifacts) cancelLive = afterPaint(live);
    });
  });
  onDestroy(() => { destroyed = true; cancelLive?.(); cancelKeep?.(); removeEventListener("pagehide", keep); removeEventListener("click", keep, true); removeEventListener("clax:people-gone", recheck); unPage?.(); life.dispose(); sync?.stop(); Q?.stop(); });
  // The footer is the page's own (view/gallery-frame.ts `FOOTER_HTML`,
  // kept in place under the gallery so it never moves): a hosted Clax's
  // Terms of Service and Privacy Policy go before its link to the source.
  $effect(() => {
    const nav = document.querySelector("nav.glegal");
    if (!legal || !nav || nav.querySelector("a.legal")) return;
    const link = (href: string, text: string) => Object.assign(document.createElement("a"), { href, textContent: text, className: "legal" });
    nav.prepend(link(legal.terms_url, "Terms of Service"), " · ", link(legal.privacy_url, "Privacy Policy"), " · ");
  });
  // Back and forward walk the tag filters.
  const popped = () => { sel = tagsFromUrl(location.search); };
</script>

<svelte:window onpopstate={popped} />

<header class="gbar">
  <a href="/" class="home" aria-label="Gallery"><Mark /></a><h1>Clax</h1><span class="sub hide-sm">{hdr ? `${hdr.site ? location.host : "local artifacts"}${me ? ` · seen as ${me}` : ""}` : ""}</span>
  <GalleryLinks current={inbox ? "inbox" : sub} inbox={!!hasInbox} {unread} queues={!!hdr?.queues} feedback={!!hdr && readConfig(hdr.feedback) !== null} member={!!hdr?.member} bot={hdr?.bot as MeBot | null | undefined} Menu={feedbackMenu.current} />
  {#if listing}<input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" value={query} oninput={e => { query = e.currentTarget.value; }} />{/if}
  <ThemeSwitch />
</header>
{#snippet usage()}{#if ban && Banner && hdr?.bot}<Banner me={hdr.bot as unknown as MeBot} />{/if}{/snippet}
{#snippet trouble()}<p class="empty" role="alert">This page could not load. <button type="button" class="ghost" onclick={() => location.reload()}>Reload to try again</button></p>{/snippet}
<main class="gal" aria-busy={pending ? "true" : undefined}>
  {#if !listing && revealed}{@render usage()}{/if}
  {#if inbox}
    {#if hasInbox && Page}<div class={revealed ? "gpage" : "gpage pending"}><Page stream={lm?.pageStream()} onready={shownPage} /></div>{/if}
    {#if hasInbox === false}<p class="empty">This browser has no inbox: open the owner's at http://localhost:{location.port}/inbox on the computer Clax runs on.</p>{:else if failed}{@render trouble()}{:else if !revealed}{@html SKELETON_HTML}{/if}
  {:else if sub}
    <div class={revealed ? "gpage" : "gpage pending"} bind:this={pageHost}></div>
    {#if failed}{@render trouble()}{:else if !revealed}{@html SKELETON_HTML}{/if}
  {:else if !ready}
    {@html SKELETON_HTML}
  {:else}
  {@render usage()}
  {#if Summary}<Summary />{/if}
  {#if error}<p class="empty">Could not load artifacts: {error}</p>{/if}
  {#snippet tile()}<NewTile me={hdr?.me?.viewer ?? null} may={hdr?.me?.may_create === true} bot={!!hdr?.bot?.enabled} />{/snippet}
  {#if artifacts && artifacts.length === 0}
    <div class="empty-gallery"><Mark size="hero" apart /><p>When an agent publishes a page, it lands here.</p><p class="muted">Or publish one yourself with <code>clax publish index.html</code>.</p><NewTile empty me={hdr?.me?.viewer ?? null} may={hdr?.me?.may_create === true} bot={!!hdr?.bot?.enabled} /></div>
  {/if}
  {#if artifacts?.length && inUse.length}
    <div class="tagbar" role="group" aria-label="Filter by tags">Tags
      {#each inUse as t (t.id)}<button type="button" class="tag" data-c={t.colour} aria-pressed={holds(sel, t)} aria-label={`Filter by tag ${t.name}`} onclick={() => setSel(toggle(sel, t))}>{t.name}</button>{/each}
      {#if sel.length}<button type="button" class="ghost tb-x" onclick={() => setSel([])}>Clear</button>{/if}
      <a class="tb-m" href="/tags">{hdr?.owner ? "Manage tags" : "All tags"}</a>
    </div>
  {/if}
  {#if artifacts && artifacts.length > 0 && shown && shown.length === 0}<p class="empty">{query.trim() ? "No artifacts match your search." : "No artifacts carry every tag chosen."}</p>{/if}
  {#snippet card(a: Artifact)}
    {@const working = feed?.byId[a.id] ?? a.working ?? []}
    {@const mk = markers(a, att[a.id], chips(working, agentNames(working, a.participants?.agents ?? [])))}
    {#snippet markerRow()}<Markers list={mk} />{/snippet}
    <GalleryCard {a} owner={!!hdr?.owner} {onTag} markers={mk.length ? markerRow : undefined} onPin={() => act(a.id, () => withToken(t => patchArtifact(a.id, { pinned: !a.pinned }, t)))}
      onDelete={() => { if (confirm(`Delete "${a.title}"? This removes every version.`)) void act(a.id, () => withToken(t => deleteArtifact(a.id, t))); }}>
      {#snippet footer()}
        <Roster people={a.participants?.people ?? []} agents={a.participants?.agents ?? []} {working} me={meId} max={3} small />
        <Seen att={att[a.id]} />
      {/snippet}
    </GalleryCard>
  {/snippet}
  {#if g?.needs.length}<NeedsGroup list={g.needs} {card} lead={tile} />{/if}
  {#if shown && shown.length > 0}
    <section class="grp rest">
      {#if !flat}<h2><span class="sw" aria-hidden="true"></span>{g?.needs.length ? REST_AFTER_NEEDS : REST_HEADING}<small>{REST_NOTE}</small></h2>{/if}
      <div class="cards">{#if !g?.needs.length}{@render tile()}{/if}{#each g?.rest ?? shown as a (a.id)}{@render card(a)}{/each}</div>
    </section>
  {/if}
  {#if artifacts && lm}<lm.HaikuLine footer />{/if}
  {/if}
</main>
