<script lang="ts">
  // The side panel's Inbox tab (spec 2026-10-06-agent-questions-and-inbox
  // §9.6): the sections of `/inbox` at the panel's width. A search box
  // (applied after a pause in typing) and **Mark all read**; **Unread**
  // lists every unread match, open questions first as cards, then rows,
  // newest first; **Read** is folded behind its count. Items stay where they
  // are as they change (a row read elsewhere, a card answered); the sections
  // are fetched again when the search changes, after a bulk mark, and for an
  // unread item they do not show. A row marks its item read and opens it: a
  // live page's item in the tab showing that page, when one does, anything
  // else on the daemon in a new tab; a question's item goes to its card when
  // it shows. The dot marks an item read or unread without opening it. Every
  // request goes through the worker. The shell's rows and cards, and their
  // styles, are reused; their links to Clax pages open through the worker.
  import { onMount } from "svelte";
  import type { InboxFilter, InboxItem, InboxPage } from "../../../shell/src/api";
  import InboxRow from "../../../shell/src/q/InboxRow.svelte";
  import QuestionList from "../../../shell/src/q/QuestionList.svelte";
  import type { PanelToWorker, WorkerToPanel } from "../messages";
  import type { InboxReq } from "./link.svelte";
  import type { PanelQuestions } from "./questions";

  type Link = {
    inbox: { owner: boolean | null; unread: number };
    questions: PanelQuestions;
    ask(m: InboxReq): Promise<WorkerToPanel>;
    post(m: PanelToWorker): void;
    onItem(f: (item: InboxItem | null) => void): () => void;
  };
  const p: { link: Link; now?: Date } = $props();

  /** How long typing waits before the search applies, and a burst of new items before the sections are fetched again. */
  const DEBOUNCE_MS = 250;
  type Section = { items: InboxItem[]; next: string | null; total: number | "10000+" };
  const fmt = (n: number | "10000+") => (n === "10000+" ? "10,000+" : n.toLocaleString("en-US"));

  let search = $state("");
  let filter: InboxFilter = {};
  let unread = $state<Section | null>(null);
  let read = $state<Section | null>(null);
  let readOpen = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  // Question items shown as cards: open when first listed, they stay cards, showing what closed them.
  let asCard = $state<string[]>([]);
  const oops = (what: string) => (e: unknown) => { error = `Could not ${what}: ${e instanceof Error ? e.message : String(e)}`; };

  const qview = (i: InboxItem) => (i.question && (p.link.questions.get(i.question.id) ?? i.question))!;
  const cards = $derived((unread?.items ?? []).filter(i => asCard.includes(i.id)).map(qview).sort((a, b) => a.created_at.localeCompare(b.created_at)));
  const rows = $derived((unread?.items ?? []).filter(i => !asCard.includes(i.id)));
  const filtered = $derived(!!search.trim());

  async function page(which: "unread" | "read", before: string | null = null): Promise<Section> {
    const r = await p.link.ask({ t: "inbox-page", filter: { ...filter, read: which }, before });
    if (r.t !== "inbox-page") throw new Error("Clax answered something else.");
    const pg: InboxPage = r.page;
    if (which === "unread") {
      asCard = [...asCard, ...pg.items.filter(i => i.kind === "question" && i.question && qview(i).status === "open" && !asCard.includes(i.id)).map(i => i.id)];
    }
    return { items: pg.items, next: pg.next_cursor, total: pg.total ?? pg.items.length };
  }

  // Each fetch is for the search at its start: an answer for an older one is dropped.
  let gen = 0;
  /** Fetches both sections; `keep` keeps the cards of questions closed here (read now, so no longer listed). */
  async function load(keep = true): Promise<void> {
    const g = ++gen;
    try {
      const [u, r] = await Promise.all([page("unread"), page("read")]);
      if (g !== gen) return;
      error = null;
      const kept = keep && unread ? cards.filter(q => q.status !== "open").map(q => unread!.items.find(i => i.question?.id === q.id)!).filter(i => i && !u.items.some(x => x.id === i.id)) : [];
      unread = { ...u, items: [...kept, ...u.items] };
      read = r;
    } catch (e) {
      if (g === gen) oops("load the inbox")(e);
    }
  }

  async function more(which: "unread" | "read"): Promise<void> {
    const s = which === "unread" ? unread : read;
    if (!s?.next) return;
    const g = gen;
    try {
      const r = await page(which, s.next);
      if (g !== gen) return;
      const next = { ...s, items: [...s.items, ...r.items.filter(i => !s.items.some(x => x.id === i.id))], next: r.next };
      if (which === "unread") unread = next; else read = next;
    } catch (e) { oops("load the inbox")(e); }
  }

  let typing: ReturnType<typeof setTimeout> | undefined;
  function onSearch(v: string): void {
    search = v;
    clearTimeout(typing);
    typing = setTimeout(() => {
      typing = undefined;
      filter = search.trim() ? { q: search.trim() } : {};
      void load(false);
    }, DEBOUNCE_MS);
  }

  /** An item as it is now, replaced where it shows; a new unread one the sections do not show fetches them again. */
  function take(item: InboxItem): void {
    const put = (s: Section | null) => (s?.items.some(x => x.id === item.id) ? { ...s, items: s.items.map(x => (x.id === item.id ? item : x)) } : null);
    const u = put(unread);
    const r = put(read);
    if (u) unread = u;
    if (r) read = r;
    if (!u && !r && !item.read) soon();
  }
  let later: ReturnType<typeof setTimeout> | undefined;
  const soon = () => { clearTimeout(later); later = setTimeout(() => { later = undefined; void load(); }, DEBOUNCE_MS); };

  async function mark(i: InboxItem, on: boolean): Promise<void> {
    const r = await p.link.ask({ t: "inbox-mark", ids: [i.id], read: on });
    if (r.t === "marked" && r.item) take(r.item);
  }
  const toggle = (i: InboxItem) => void mark(i, !i.read).catch(oops("mark it"));

  async function markAll(): Promise<void> {
    busy = true;
    try {
      // Up to the newest item shown, so one made since stays unread.
      await p.link.ask({ t: "inbox-mark-all", filter: filtered ? { ...filter } : null, upto: Math.max(...unread!.items.map(i => i.seq)) });
      await load();
    } catch (e) {
      oops("mark them read")(e);
    }
    busy = false;
  }

  let root: HTMLElement | undefined = $state();
  /** Scrolls to question `qid`'s card and focuses it; false when none shows it. */
  const toCard = (qid: string): boolean => {
    const el = [...(root?.querySelectorAll<HTMLElement>("[data-question]") ?? [])].find(e => e.dataset.question === qid);
    el?.scrollIntoView?.({ block: "center" });
    el?.focus({ preventScroll: true });
    return !!el;
  };
  /** Where an item leads: its card when it is a question shown here, else through the worker. */
  const go = (url: string) => {
    const q = /^\/inbox\?q=([^&]+)$/.exec(url);
    if (!q || !toCard(decodeURIComponent(q[1]))) p.link.post({ t: "open-url", url });
  };
  const open = (i: InboxItem) => {
    // A failed mark does not keep the person from the item.
    const marked = i.read ? Promise.resolve() : mark(i, true).catch(() => {});
    void marked.then(() => go(i.url));
  };
  // A card's links to Clax pages are daemon paths: the worker opens them.
  function onClick(e: MouseEvent): void {
    const a = (e.target as Element | null)?.closest?.("a[href]");
    const href = a?.getAttribute("href");
    if (!href?.startsWith("/") || href.startsWith("//")) return;
    e.preventDefault();
    go(href);
  }

  onMount(() => {
    const off = p.link.onItem(item => { if (item) take(item); else if (unread) soon(); });
    void load();
    return () => { off(); clearTimeout(typing); clearTimeout(later); };
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<!-- Links keep their own keys: Enter on a link is a click, which this hears. -->
<div class="inbox-tab" bind:this={root} onclick={onClick}>
  <div class="ibar">
    <input type="search" placeholder="Search the inbox" aria-label="Search the inbox" value={search} oninput={e => onSearch(e.currentTarget.value)} />
    <button type="button" disabled={busy || !unread?.items.some(i => !i.read)} onclick={markAll}>Mark all read</button>
  </div>
  {#if error}<p class="err" role="alert">{error}</p>{/if}

  <section class="sec" aria-labelledby="ptab-unread-h">
    <h2 id="ptab-unread-h">Unread{#if unread}<span class="c">{fmt(unread.total)}</span>{/if}</h2>
    {#if !unread}
      {#if !error}<p class="note">Loading…</p>{/if}
    {:else}
      <QuestionList list={cards} feed={p.link.questions} now={p.now} />
      {#if rows.length}
        <ul class="rows">{#each rows as i (i.id)}<InboxRow item={i} others={unread.items} now={p.now} onOpen={open} onToggle={toggle} />{/each}</ul>
      {/if}
      {#if !unread.items.length}<p class="note">{filtered ? "No unread items match." : "Nothing unread."}</p>{/if}
      {#if unread.next}<button type="button" class="more" onclick={() => more("unread")}>Show more</button>{/if}
    {/if}
  </section>

  {#if read && read.total !== 0}
    <section class="sec" aria-label="Read">
      <button type="button" class="fold" aria-expanded={readOpen} aria-controls="ptab-read" onclick={() => { readOpen = !readOpen; }}>
        {readOpen ? "Hide read items" : `Show ${fmt(read.total)} read ${read.total === 1 ? "item" : "items"}`}
      </button>
      {#if readOpen}
        <div id="ptab-read">
          <ul class="rows">{#each read.items as i (i.id)}<InboxRow item={i} others={read.items} now={p.now} onOpen={open} onToggle={toggle} />{/each}</ul>
          {#if read.next}<button type="button" class="more" onclick={() => more("read")}>Show more</button>{/if}
        </div>
      {/if}
    </section>
  {/if}
</div>

<style>
  .inbox-tab { padding: 12px var(--gutter) 18px; min-width: 0; }
  .ibar { display: flex; gap: 8px; }
  .ibar input { flex: 1; min-width: 0; font-size: 13px; }
  .ibar button { flex: none; font-size: 12.5px; }
  .err { margin: 10px 0 0; color: var(--danger); font-size: 13px; overflow-wrap: anywhere; }
  .sec { margin-top: 16px; min-width: 0; }
  h2 { display: flex; align-items: baseline; gap: 8px; margin: 0 0 8px; font-size: 13px; font-weight: 600; }
  h2 .c { font: 400 12px var(--mono); color: var(--muted); font-variant-numeric: tabular-nums; }
  .sec :global(.qlist) { margin-bottom: 8px; }
  .rows { margin: 0; padding: 0; }
  .note { margin: 6px 0; color: var(--muted); font-size: 13px; }
  .more { margin-top: 8px; font-size: 12.5px; }
  .fold { width: 100%; justify-content: flex-start; background: none; border: 1px dashed var(--border-hover); color: var(--muted); font-weight: 500; font-size: 12.5px; }
  .fold[aria-expanded="true"] { margin-bottom: 6px; }
</style>
