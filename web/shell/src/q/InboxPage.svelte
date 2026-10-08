<svelte:options css="injected" />

<script lang="ts">
  // `/inbox` (spec 2026-10-06-agent-questions-and-inbox §9.2): everything
  // agents sent back. The header has the unread count, **Mark all read** and
  // **Notify me** (while the browser has not been asked; a quiet line when
  // notifications are blocked). Search text and filters (kind, page, agent,
  // dates) apply to both sections and are kept in the URL. **Unread** lists
  // every unread match, open questions first as cards, then rows, newest
  // first; **Read** is folded behind its count and pages 50 at a time.
  // Items stay where they are as they change (a row read elsewhere, a card
  // answered): the sections are fetched again when the search changes, on a
  // bulk mark, and when the stream resyncs. A row opens its item and marks
  // it read; its dot marks it without opening. `?q=<question>` scrolls to
  // that question's card and focuses it. At phone width the filters sit
  // behind **Filters**.
  import { onMount } from "svelte";
  import { type InboxFilter, type InboxItem, type InboxKind, listArtifacts } from "../api";
  import { after } from "../clock";
  import type { InboxFeed, QuestionFeed } from "./feed.svelte";
  import { INBOX_KINDS, filterFromUrl, filterToUrl, newestSeq } from "./inbox-model";
  import InboxRow from "./InboxRow.svelte";
  import { openItem } from "./open";
  import QuestionList from "./QuestionList.svelte";
  import { shared } from "./shared";

  type Props = {
    inbox?: InboxFeed;
    questions?: QuestionFeed;
    /** Goes to an item's URL (default: the browser navigates). */
    go?: (url: string) => void;
    now?: Date;
  };
  const p: Props = $props();
  const inbox = $derived(p.inbox ?? shared().inbox);
  const questions = $derived(p.questions ?? shared().questions);

  /** How long typing waits before the search applies. */
  const DEBOUNCE_MS = 250;
  const KIND_LABEL: Record<InboxKind, string> = { reply: "Replies", version: "Versions", published: "Published", question: "Questions", finished: "Finished" };
  const fmt = (n: number | "10000+") => (n === "10000+" ? "10,000+" : n.toLocaleString("en-US"));
  /** Says what failed: `what` ("load the inbox"), and why. */
  const oops = (what: string) => (e: unknown) => { error = `Could not ${what}: ${e instanceof Error ? e.message : String(e)}`; };
  const loadFailed = oops("load the inbox");

  type Section = { items: InboxItem[]; next: string | null; total: number | "10000+" };
  const initial = filterFromUrl(location.search);
  let filter = $state<InboxFilter>(initial);
  let search = $state(initial.q ?? "");
  let target = new URLSearchParams(location.search).get("q");
  let unread = $state<Section | null>(null);
  // The read section: its count always, its items once unfolded.
  let readCount = $state<number | "10000+">(0);
  let readOpen = $state(false);
  let read = $state<Section | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let filtersOpen = $state(false);
  // Question items shown as cards: open when first listed, they stay cards, showing what closed them.
  let asCard = $state<string[]>([]);
  const N = typeof Notification === "undefined" ? undefined : Notification;
  let perm = $state(N?.permission);

  const filtered = $derived([filter.q, filter.kind?.length, filter.artifact, filter.agent, filter.since, filter.until].some(Boolean));
  // A card's view: the question feed's (live), else the item's.
  const qview = (i: InboxItem) => (i.question && (questions.get(i.question.id) ?? i.question))!;
  const cards = $derived((unread?.items ?? []).filter(i => asCard.includes(i.id)).map(qview).sort((a, b) => a.created_at.localeCompare(b.created_at)));
  const rows = $derived((unread?.items ?? []).filter(i => !asCard.includes(i.id)));
  // Harnesses and agents seen, as [value, label].
  // The pickers' options, [value, label]: the pages and the harnesses and
  // agents of the items listed, and the one chosen.
  const listed = $derived([...(unread?.items ?? []), ...(read?.items ?? [])]);
  // Every artifact, and the pages of the items listed (live pages among them).
  let artifacts = $state<string[][]>([]);
  const pages = $derived([...new Map([...(filter.artifact ? [[filter.artifact, filter.artifact]] : []), ...artifacts, ...listed.flatMap(i => (i.artifact ? [[i.artifact.id, i.artifact.title ?? i.artifact.id]] : []))] as [string, string][])]);
  const agents = $derived.by(() => {
    const seen = new Map<string, string>(filter.agent ? [[filter.agent, filter.agent]] : []);
    for (const { agent: a } of listed) if (a) seen.set(a.harness, a.harness).set(a.handle, `${a.harness} ${a.handle.slice(2, 6)} · ${a.project}`);
    return [...seen];
  });
  const cardsFrom = (items: InboxItem[]) => {
    asCard = [...asCard, ...items.filter(i => i.kind === "question" && i.question && qview(i).status === "open" && !asCard.includes(i.id)).map(i => i.id)];
  };

  // Each fetch is for the search at its start: an answer for an older one is dropped.
  let gen = 0;
  /** A page of `which` for the search, after `before`. */
  const fetch = async (which: "unread" | "read", before?: string | null, limit?: number): Promise<Section> => {
    const r = await inbox.page({ ...filter, read: which }, before, limit);
    if (which === "unread") cardsFrom(r.items);
    return { items: r.items, next: r.next_cursor, total: r.total ?? r.items.length };
  };

  /** Fetches both sections; `keep` keeps the cards of questions closed
   * here (read now, so no longer listed), for the same search. */
  async function load(keep = true): Promise<void> {
    const g = ++gen;
    try {
      const [u, r, shown] = await Promise.all([fetch("unread"), fetch("read", null, 1), readOpen ? fetch("read") : null]);
      if (g !== gen) return;
      error = null;
      const kept = keep ? cards.filter(q => q.status !== "open").map(q => unread!.items.find(i => i.question?.id === q.id)!).filter(i => !u.items.some(x => x.id === i.id)) : [];
      unread = { ...u, items: [...kept, ...u.items] };
      readCount = r.total;
      read = shown;
      // `?q=`: that question's card takes focus, once.
      const t = target;
      if (t) queueMicrotask(() => { if (toCard(t)) target = null; });
    } catch (e) {
      if (g === gen) loadFailed(e);
    }
  }

  /** The next page of a section, appended. */
  async function more(which: "unread" | "read"): Promise<void> {
    const s = which === "unread" ? unread : read;
    if (!s?.next) return;
    const g = gen;
    try {
      const r = await fetch(which, s.next);
      if (g !== gen) return;
      const next = { ...s, items: [...s.items, ...r.items.filter(i => !s.items.some(x => x.id === i.id))], next: r.next };
      if (which === "unread") unread = next; else read = next;
    } catch (e) { loadFailed(e); }
  }

  async function toggleRead(): Promise<void> {
    readOpen = !readOpen;
    if (!readOpen || read) return;
    const g = gen;
    try {
      const r = await fetch("read");
      if (g === gen && readOpen) read = r;
    } catch (e) { loadFailed(e); }
  }

  /** Scrolls to question `qid`'s card and focuses it; false when no card shows it. */
  const toCard = (qid: string): boolean => {
    const el = [...document.querySelectorAll<HTMLElement>(".inbox [data-question]")].find(e => e.dataset.question === qid);
    el?.scrollIntoView?.({ block: "center" });
    el?.focus({ preventScroll: true });
    return !!el;
  };

  /** Applies a new search: into the URL, then both sections again. */
  function apply(next: InboxFilter): void {
    filter = next;
    history.replaceState(history.state, "", `/inbox${filterToUrl(next)}`);
    read = null;
    void load(false);
  }

  let typing: (() => void) | undefined;
  function onSearch(v: string): void {
    search = v;
    typing?.();
    typing = after(DEBOUNCE_MS, () => { typing = undefined; apply({ ...filter, q: search.trim() || undefined }); });
  }
  const toggleKind = (k: InboxKind) => {
    const on = filter.kind ?? [];
    apply({ ...filter, kind: INBOX_KINDS.filter(x => (x === k) !== on.includes(x)) });
  };
  const set = (k: "artifact" | "agent" | "since" | "until", v: string) => apply({ ...filter, [k]: v || undefined });

  async function markAll(): Promise<void> {
    busy = true;
    try {
      // Up to the newest item shown, so one made since stays unread.
      await inbox.markAll(filtered ? { ...filter } : undefined, newestSeq(unread!.items));
      await load();
    } catch (e) {
      oops("mark them read")(e);
    }
    busy = false;
  }

  // Where an item leads; a question's (`/inbox?q=…`) is this page, whose card, if shown, takes focus.
  const go = (url: string) => {
    const m = /^\/inbox\?q=([^&]+)$/.exec(url);
    if (m) toCard(decodeURIComponent(m[1]));
    else (p.go ?? (u => location.assign(u)))(url);
  };
  const open = (i: InboxItem) => void openItem(inbox, i, go);
  const toggle = (i: InboxItem) => void inbox.mark(i.id, !i.read).catch(oops("mark it"));
  // Asked only from this click (spec §9.7).
  const askNotify = () => void N?.requestPermission().then(r => { perm = r; }, () => {});

  // An item as it is now: replaced where it shows; for a new unread one the
  // sections are fetched again (debounced, so a burst is one fetch).
  let refetchSoon: (() => void) | undefined;
  const soon = () => { refetchSoon?.(); refetchSoon = after(DEBOUNCE_MS, () => { refetchSoon = undefined; void load(); }); };
  function take(item: InboxItem): void {
    const put = (s: Section | null) => (s?.items.some(x => x.id === item.id) ? { ...s, items: s.items.map(x => (x.id === item.id ? item : x)) } : null);
    const u = put(unread);
    const r = put(read);
    if (u) unread = u;
    if (r) read = r;
    if (!u && !r && !item.read) soon();
  }

  onMount(() => {
    document.title = "Inbox · Clax";
    void listArtifacts().then(a => { artifacts = a.map(x => [x.id, x.title]); }, () => {});
    // Fetched once on open: now, or when the inbox topic first goes live.
    const off = inbox.listen(c => { if ("item" in c) take(c.item); else if (unread) soon(); else void load(); });
    if (!inbox.waiting) void load();
    return () => { off(); typing?.(); refetchSoon?.(); };
  });
</script>

<div class="inbox">
  <header class="ihead">
    <h2>Inbox</h2>
    <span class="icnt">{inbox.unread.toLocaleString("en-US")} unread</span>
    <span class="sp"></span>
    {#if perm === "default"}<button type="button" onclick={askNotify}>Notify me</button>{/if}
    <button type="button" class="primary" disabled={busy || !unread?.items.some(i => !i.read)} onclick={markAll}>Mark all read</button>
  </header>
  {#if perm === "denied"}<p class="quiet">Notifications are blocked for this site. The unread count shows in the tab's title.</p>{/if}

  <div class="find">
    <input type="search" class="isearch" placeholder="Search the inbox" aria-label="Search the inbox" value={search} oninput={e => onSearch(e.currentTarget.value)} />
    <button type="button" class="filters-toggle" aria-expanded={filtersOpen} aria-controls="inbox-filters" onclick={() => { filtersOpen = !filtersOpen; }}>Filters</button>
  </div>
  <div class="filters" id="inbox-filters" data-open={filtersOpen ? "" : undefined}>
    <div class="kinds" role="group" aria-label="Kinds">
      {#each INBOX_KINDS as k (k)}
        <button type="button" class="kind" aria-pressed={!!filter.kind?.includes(k)} onclick={() => toggleKind(k)}>{KIND_LABEL[k]}</button>
      {/each}
    </div>
    <label class="pick">Page
      <select onchange={e => set("artifact", e.currentTarget.value)}>
        <option value="">Any page</option>
        {#each pages as a (a[0])}<option value={a[0]} selected={a[0] === filter.artifact}>{a[1]}</option>{/each}
      </select>
    </label>
    <label class="pick">Agent
      <select onchange={e => set("agent", e.currentTarget.value)}>
        <option value="">Any agent</option>
        {#each agents as a (a[0])}<option value={a[0]} selected={a[0] === filter.agent}>{a[1]}</option>{/each}
      </select>
    </label>
    <label class="pick">From <input type="date" value={filter.since ?? ""} onchange={e => set("since", e.currentTarget.value)} /></label>
    <label class="pick">To <input type="date" value={filter.until ?? ""} onchange={e => set("until", e.currentTarget.value)} /></label>
  </div>

  {#if error}<p class="err" role="alert">{error}</p>{/if}

  <section class="sec" aria-labelledby="inbox-unread-h">
    <h3 id="inbox-unread-h">Unread{#if unread}<span class="c">{fmt(unread.total)}</span>{/if}</h3>
    {#if !unread}
      {#if !error}<p class="note">Loading…</p>{/if}
    {:else}
      <QuestionList list={cards} feed={questions} now={p.now} />
      {#if rows.length}
        <ul class="rows">{#each rows as i (i.id)}<InboxRow item={i} others={unread.items} now={p.now} onOpen={open} onToggle={toggle} />{/each}</ul>
      {/if}
      {#if !unread.items.length}
        <p class="note">{filtered ? "No unread items match." : "Nothing unread."}</p>
      {/if}
      {#if unread.next}<button type="button" class="more" onclick={() => more("unread")}>Show more</button>{/if}
    {/if}
  </section>

  {#if unread && readCount !== 0}
    <section class="sec" aria-label="Read">
      <button type="button" class="fold" aria-expanded={readOpen} aria-controls="inbox-read" onclick={toggleRead}>
        {readOpen ? "Hide read items" : `Show ${fmt(readCount)} read ${readCount === 1 ? "item" : "items"}`}
      </button>
      {#if readOpen}
        <div id="inbox-read">
          {#if read}
            <ul class="rows">{#each read.items as i (i.id)}<InboxRow item={i} others={read.items} now={p.now} onOpen={open} onToggle={toggle} />{/each}</ul>
            {#if read.next}<button type="button" class="more" onclick={() => more("read")}>Show more</button>{/if}
          {:else}<p class="note">Loading…</p>{/if}
        </div>
      {/if}
    </section>
  {/if}
</div>

<style>
  :global {
    .inbox { max-width: 880px; margin: 0 auto; min-width: 0; }
    .inbox .ihead { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
    .inbox .ihead h2 { margin: 0; font-size: 22px; line-height: 1.2; }
    .inbox .icnt { color: var(--muted); font-size: 13px; font-variant-numeric: tabular-nums; }
    .inbox .sp { flex: 1; }
    .inbox .quiet { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); }
    .inbox .find { display: flex; gap: 8px; margin-top: 14px; }
    .inbox .isearch { flex: 1; min-width: 0; }
    .inbox .filters-toggle { display: none; }
    .inbox .filters { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 14px; margin-top: 10px; }
    .inbox .kinds { display: flex; flex-wrap: wrap; gap: 6px; }
    .inbox .kind { min-height: 28px; padding: 3px 11px; border-radius: 999px; font: 500 12.5px var(--font); color: var(--muted); background: var(--card); border: 1px solid var(--border-hover); }
    .inbox .kind[aria-pressed="true"] { color: var(--fg); background: var(--hover); border-color: var(--fg); }
    .inbox .pick { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; color: var(--muted); }
    .inbox .pick select { max-width: 220px; }
    .inbox .err { margin: 12px 0 0; color: var(--danger); font-size: 13px; }
    .inbox .sec { margin-top: 22px; min-width: 0; }
    .inbox .sec h3 { display: flex; align-items: baseline; gap: 8px; margin: 0 0 10px; font-size: 15px; }
    .inbox .sec h3 .c { font: 400 12.5px var(--mono); color: var(--muted); font-variant-numeric: tabular-nums; }
    .inbox .sec .qlist { margin-bottom: 10px; }
    .inbox .rows { margin: 0; padding: 0; }
    .inbox .note { margin: 8px 0; color: var(--muted); font-size: 13px; }
    .inbox .more { margin-top: 10px; }
    .inbox .fold { width: 100%; justify-content: flex-start; background: none; border: 1px dashed var(--border-hover); color: var(--muted); font-weight: 500; }
    .inbox .fold[aria-expanded="true"] { margin-bottom: 8px; }
    @media (max-width: 700px) {
      .inbox .filters-toggle { display: inline-flex; }
      .inbox .filters { flex-direction: column; align-items: stretch; }
      .inbox .filters:not([data-open]) { display: none; }
      .inbox .pick { justify-content: space-between; }
      .inbox .pick select, .inbox .pick input { flex: 1; max-width: none; min-width: 0; }
    }
  }
</style>
