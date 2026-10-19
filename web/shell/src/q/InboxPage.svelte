<svelte:options css="injected" />

<script lang="ts">
  // `/inbox` (spec 2026-10-06-agent-questions-and-inbox §9.2): everything
  // agents sent back. The header has the unread count, **Mark all read** and
  // **Notify me** (while the browser has not been asked; a quiet line when
  // notifications are blocked). Search text and filters (kind, page, agent,
  // dates) apply to both sections and are kept in the URL. **Unread** shows
  // every open question as a card, then approval requests (spec
  // 2026-10-08-multi-agent-multi-person §9.5) and connection requests
  // (§3.2) as cards, then the other unread items grouped by the page they
  // are about (`InboxGroup`), newest group first, items about no page in
  // one group, "Other". A group says how many of its items are new and
  // how old its newest is, lists its three newest, pages the rest with
  // **Show more**, and marks its new items read with **Mark read**; one
  // with something new starts open, the rest folded. **Read** is folded
  // behind its count, its groups folded the same way (open when there is
  // one, as with the Page filter), a decided request a card in its group.
  // Items stay where they are as they change (a row read elsewhere, a card
  // answered): the sections are fetched again when the search changes, on a
  // bulk mark, and when the stream resyncs. A row opens its item and marks
  // it read; its dot marks it without opening. `?q=<question>` scrolls to
  // that question's card and focuses it, and `?approval=<request>` to
  // that request's. A decided card stays where it is, saying how, and the
  // read section leaves out what Unread still shows. A machine that
  // connected with an invite and an agent that ended are rows.
  // Only the owner's browser offers the Connections filter. At phone width
  // the filters sit behind **Filters**. The agent strip (§13) opens the
  // page, under its header: a chip filters the inbox by its agent; it is
  // fetched again with the sections.
  import { onMount } from "svelte";
  import { type InboxFilter, type InboxItem, type InboxKind, type StripAgent, getToken, listArtifacts } from "../api";
  import AgentStrip from "../people/AgentStrip.svelte";
  import { ticker } from "../ui/ticker.svelte";
  import { after } from "../clock";
  import type { InboxFeed, QuestionFeed } from "./feed.svelte";
  import { approvalOf, newestSeq } from "./inbox-model";
  import { INBOX_KINDS, filterFromUrl, filterToUrl } from "./inbox-filter";
  import { GROUP_ROWS, type Group, MORE_ROWS, appendGroups, appendRows, fromWire, groupKey, lists, takeItem } from "./inbox-groups";
  import ApprovalCard from "../people/ApprovalCard.svelte";
  import ConnectionCard from "../people/ConnectionCard.svelte";
  import InboxGroup from "./InboxGroup.svelte";
  import InboxRow from "./InboxRow.svelte";
  import { inboxAgents } from "./api";
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
  /** The kinds shown as cards above the groups, and how many such items one fetch lists. */
  const CARD_KINDS: InboxKind[] = ["question", "approval", "connection"];
  const MAX_CARDS = 200;
  const KIND_LABEL: Record<InboxKind, string> = { reply: "Replies", version: "Versions", published: "Published", question: "Questions", finished: "Finished", approval: "Approvals", connection: "Connections", ended: "Ended", push: "Pushes" };
  const fmt = (n: number | "10000+") => (n === "10000+" ? "10,000+" : n.toLocaleString("en-US"));
  /** Says what failed: `what` ("load the inbox"), and why. */
  const oops = (what: string) => (e: unknown) => { error = `Could not ${what}: ${e instanceof Error ? e.message : String(e)}`; };
  const loadFailed = oops("load the inbox");

  type Which = "unread" | "read";
  /** A section's groups, the cursor of its next page of groups, and its match count. */
  type Section = { groups: Group[]; next: string | null; total: number | "10000+" };
  const initial = filterFromUrl(location.search);
  let filter = $state<InboxFilter>(initial);
  let search = $state(initial.q ?? "");
  const asked = new URLSearchParams(location.search);
  // The card to focus once listed: a question's (`?q=`) or an approval request's (`?approval=`).
  let target: { q: string } | { approval: string } | null = asked.get("q") ? { q: asked.get("q")! } : asked.get("approval") ? { approval: asked.get("approval")! } : null;
  let unread = $state<Section | null>(null);
  // The unread section's cards: open questions and decisions, and those closed or decided here.
  let cardItems = $state<InboxItem[]>([]);
  // The read section: its count always, its groups once unfolded.
  let readCount = $state<number | "10000+">(0);
  let readOpen = $state(false);
  let read = $state<Section | null>(null);
  // Whether a group (`<section>:<key>`) is open: as the person set it, else as it was when listed.
  let toggled = $state<Record<string, boolean>>({});
  let listedOpen = $state<Record<string, boolean>>({});
  let busy = $state(false);
  let error = $state<string | null>(null);
  let filtersOpen = $state(false);
  // The owner's browser (the token is served): connection requests are the owner's alone, so only theirs filters by them.
  let owner = $state(false);
  // Question items shown as cards: open when first listed, they stay cards, showing what closed them.
  let asCard = $state<string[]>([]);
  // The agent strip, and the clock its block times read (each 30 s).
  let strip = $state<StripAgent[]>([]);
  const clock = ticker(() => false, () => p.now, 30_000);
  const loadStrip = () => inboxAgents().then(b => { if (b !== "forbidden" && Array.isArray(b.agents)) strip = b.agents; }, () => {});
  const N = typeof Notification === "undefined" ? undefined : Notification;
  let perm = $state(N?.permission);

  const filtered = $derived([filter.q, filter.kind?.length, filter.artifact, filter.agent, filter.since, filter.until].some(Boolean));
  // A card's view: the question feed's (live), else the item's.
  const qview = (i: InboxItem) => (i.question && (questions.get(i.question.id) ?? i.question))!;
  const cards = $derived(cardItems.filter(i => asCard.includes(i.id)).map(qview).sort((a, b) => a.created_at.localeCompare(b.created_at)));
  /** Whether `i` is a decision's card: an approval request, or a machine's request to connect. */
  const decision = (i: InboxItem) => i.kind === "approval" || (i.kind === "connection" && i.connection?.via !== "invite");
  const decisions = $derived(cardItems.filter(decision).sort((a, b) => a.seq - b.seq));
  // What the unread section shows as a card, so no group lists it as a row.
  const carded = $derived(new Set([...asCard, ...decisions.map(i => i.id)]));
  const rowsOf = (g: Group) => g.items.filter(i => !carded.has(i.id));
  /** How many of a group's items are new: its unread count, less its unread items shown as cards. */
  const freshOf = (g: Group) => Math.max(0, g.unread - cardItems.filter(i => !i.read && carded.has(i.id) && groupKey(i) === g.key).length);
  // A group whose every item is a card has nothing of its own to show.
  const unreadGroups = $derived((unread?.groups ?? []).filter(g => rowsOf(g).length || freshOf(g) || g.next));
  // The read section leaves out what Unread still shows (a card kept there once decided).
  const inUnread = $derived(new Set([...cardItems, ...(unread?.groups ?? []).flatMap(g => g.items)].map(i => i.id)));
  const readRowsOf = (g: Group) => g.items.filter(i => !inUnread.has(i.id));
  const readGroups = $derived((read?.groups ?? []).filter(g => readRowsOf(g).length || g.next));
  const isOpen = (which: Which, g: Group) => toggled[`${which}:${g.key}`] ?? listedOpen[`${which}:${g.key}`] ?? false;
  /** Notes whether each group listed starts open: one with something new, or a section's only group. */
  const opening = (which: Which, gs: Group[], one: boolean) => {
    listedOpen = { ...listedOpen, ...Object.fromEntries(gs.map(g => [`${which}:${g.key}`, (which === "unread" && freshOf(g) > 0) || one])) };
  };
  // The pickers' options, [value, label]: the pages and the harnesses and
  // agents of the items listed, and the one chosen.
  const listed = $derived([...cardItems, ...[...(unread?.groups ?? []), ...(read?.groups ?? [])].flatMap(g => g.items)]);
  // Every artifact, and the pages of the items listed (live pages among them).
  let artifacts = $state<string[][]>([]);
  const pages = $derived([...new Map([...(filter.artifact ? [[filter.artifact, filter.artifact]] : []), ...artifacts, ...listed.flatMap(i => (i.artifact ? [[i.artifact.id, i.artifact.title ?? i.artifact.id]] : []))] as [string, string][])]);
  const agents = $derived.by(() => {
    const seen = new Map<string, string>(filter.agent ? [[filter.agent, filter.agent]] : []);
    // An agent by its name (spec 2026-10-08 §11), with its person when it is not the reader's; one with none by harness and handle.
    for (const { agent: a } of listed) if (a) seen.set(a.harness, `All ${a.harness} agents`).set(a.handle, a.name ? `${a.name}${a.own === false && a.person_name ? ` · ${a.person_name}` : ""}` : `${a.harness} ${a.handle.slice(2, 6)}${a.project ? ` · ${a.project}` : ""}`);
    return [...seen];
  });
  // Something unread Mark all read would read: an open approval request stays unread until decided.
  const markable = $derived(unreadGroups.some(g => freshOf(g) > 0) || cardItems.some(i => !i.read && approvalOf(i)?.status !== "open"));
  const cardsFrom = (items: InboxItem[]) => {
    asCard = [...asCard, ...items.filter(i => i.kind === "question" && i.question && qview(i).status === "open" && !asCard.includes(i.id)).map(i => i.id)];
  };

  // Each fetch is for the search at its start: an answer for an older one is dropped.
  let gen = 0;
  /** The unread items shown as cards: open questions and decisions. */
  const fetchCards = async (): Promise<InboxItem[]> => {
    const kind = filter.kind?.length ? CARD_KINDS.filter(k => filter.kind!.includes(k)) : CARD_KINDS;
    if (!kind.length) return [];
    const r = await inbox.page({ ...filter, kind, read: "unread" }, null, MAX_CARDS);
    cardsFrom(r.items);
    return r.items.filter(i => asCard.includes(i.id) || decision(i));
  };
  /** A page of `which`'s groups, after `before`. */
  const fetchGroups = async (which: Which, before?: string | null): Promise<Section> => {
    const r = await inbox.groups({ ...filter, read: which }, before, GROUP_ROWS);
    const groups = r.groups.map(fromWire);
    return { groups, next: r.next_cursor, total: r.total ?? groups.reduce((n, g) => n + g.total, 0) };
  };
  /** How many read items match. */
  const countRead = async () => { const r = await inbox.page({ ...filter, read: "read" }, null, 1); return r.total ?? r.items.length; };

  /** Fetches both sections; `keep` keeps the cards of questions closed
   * here (read now, so no longer listed), for the same search. */
  async function load(keep = true): Promise<void> {
    const g = ++gen;
    void loadStrip();
    try {
      const [c, u, r, shown] = await Promise.all([fetchCards(), fetchGroups("unread"), countRead(), readOpen ? fetchGroups("read") : null]);
      if (g !== gen) return;
      error = null;
      // A question's card closed here, and a decision's card decided here: read now, no longer listed as unread.
      const kept = keep ? [
        ...cards.filter(q => q.status !== "open").map(q => cardItems.find(i => i.question?.id === q.id)!),
        ...decisions.filter(i => i.read),
      ].filter(i => i && !c.some(x => x.id === i.id)) : [];
      cardItems = [...kept, ...c];
      unread = u;
      opening("unread", u.groups, false);
      readCount = r;
      read = shown;
      if (shown) opening("read", shown.groups, shown.groups.length === 1);
      void fill(g);
      // `?q=` or `?approval=`: that card takes focus, once.
      const t = target;
      if (t) queueMicrotask(() => { if ("q" in t ? toCard(t.q) : toCard(t.approval, "approval")) target = null; });
    } catch (e) {
      if (g === gen) loadFailed(e);
    }
  }

  /** Lists more of each open unread group whose first rows are all cards. */
  async function fill(g: number): Promise<void> {
    for (const x of unreadGroups) if (g === gen && x.next && rowsOf(x).length === 0 && isOpen("unread", x)) await more("unread", x.key);
  }

  /** The next rows of group `key` of `which`, appended. */
  async function more(which: Which, key: string): Promise<void> {
    const at = (which === "unread" ? unread : read)?.groups.find(x => x.key === key);
    if (!at?.next) return;
    const g = gen;
    try {
      const r = await inbox.page({ ...filter, read: which, artifact: key }, at.next, MORE_ROWS);
      const s = which === "unread" ? unread : read;
      if (g !== gen || !s) return;
      const next = { ...s, groups: appendRows(s.groups, key, r.items, r.next_cursor) };
      if (which === "unread") unread = next; else read = next;
    } catch (e) { loadFailed(e); }
  }

  /** The next page of `which`'s groups, appended. */
  async function moreGroups(which: Which): Promise<void> {
    const s = which === "unread" ? unread : read;
    if (!s?.next) return;
    const g = gen;
    try {
      const r = await fetchGroups(which, s.next);
      const now = which === "unread" ? unread : read;
      if (g !== gen || !now) return;
      opening(which, r.groups, false);
      const next = { ...now, groups: appendGroups(now.groups, r.groups), next: r.next };
      if (which === "unread") unread = next; else read = next;
    } catch (e) { loadFailed(e); }
  }

  const flip = (which: Which, g: Group) => { toggled = { ...toggled, [`${which}:${g.key}`]: !isOpen(which, g) }; };

  async function toggleRead(): Promise<void> {
    readOpen = !readOpen;
    if (!readOpen || read) return;
    const g = gen;
    try {
      const r = await fetchGroups("read");
      if (g === gen && readOpen) { opening("read", r.groups, r.groups.length === 1); read = r; }
    } catch (e) { loadFailed(e); }
  }

  /** Scrolls to question `id`'s card (or approval request `id`'s) and focuses it; false when no card shows it. */
  const toCard = (id: string, kind: "question" | "approval" = "question"): boolean => {
    const el = [...document.querySelectorAll<HTMLElement>(`.inbox [data-${kind}]`)].find(e => e.dataset[kind] === id);
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

  /** Marks read the unread items matching `f` up to the newest of `shown`, then fetches the sections again. */
  async function markUpTo(f: InboxFilter | undefined, shown: InboxItem[]): Promise<void> {
    busy = true;
    try {
      // Up to the newest item shown, so one made since stays unread.
      await inbox.markAll(f, newestSeq(shown));
      await load();
    } catch (e) {
      oops("mark them read")(e);
    }
    busy = false;
  }
  const markAll = () => markUpTo(filtered ? { ...filter } : undefined, [...cardItems, ...(unread?.groups ?? []).flatMap(g => g.items)]);
  /** A group's Mark read: its unread items, up to its newest shown. */
  const markGroup = (g: Group) => markUpTo({ ...filter, artifact: g.key }, g.items);

  // Where an item leads; a question's (`/inbox?q=…`) is this page, whose card, if shown, takes focus.
  const go = (url: string) => {
    const m = /^\/inbox\?(q|approval)=([^&]+)$/.exec(url);
    if (m) toCard(decodeURIComponent(m[2]), m[1] === "q" ? "question" : "approval");
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
  // An item that changed may change a block: the strip is fetched again (debounced).
  let stripSoon: (() => void) | undefined;
  function take(item: InboxItem): void {
    stripSoon?.();
    stripSoon = after(DEBOUNCE_MS, () => { stripSoon = undefined; void loadStrip(); });
    // An item Unread shows that was read (a decision, a mark elsewhere), or
    // unread again, moves the counts with it: its section's and its group's.
    const was = cardItems.find(x => x.id === item.id) ?? unread?.groups.flatMap(g => g.items).find(x => x.id === item.id);
    const d = was && was.read !== item.read ? (item.read ? -1 : 1) : 0;
    if (d && unread && typeof unread.total === "number") unread = { ...unread, total: Math.max(0, unread.total + d) };
    if (d && typeof readCount === "number") readCount = Math.max(0, readCount - d);
    if (cardItems.some(x => x.id === item.id)) cardItems = cardItems.map(x => (x.id === item.id ? item : x));
    if (unread) unread = { ...unread, groups: takeItem(unread.groups, item, d) };
    if (read) read = { ...read, groups: takeItem(read.groups, item, 0) };
    if (!was && !lists(read?.groups, item.id) && !item.read) soon();
  }

  onMount(() => {
    document.title = "Inbox · Clax";
    void getToken().then(t => { owner = !!t; });
    void listArtifacts().then(a => { artifacts = a.map(x => [x.id, x.title]); }, () => {});
    // Fetched once on open: now, or when the inbox topic first goes live.
    const off = inbox.listen(c => { if ("item" in c) take(c.item); else if (unread) soon(); else void load(); });
    if (!inbox.waiting) void load();
    return () => { off(); typing?.(); refetchSoon?.(); stripSoon?.(); };
  });
</script>

{#snippet decided(list: InboxItem[])}
  {#if list.length}
    <div class="dlist">{#each list as i (i.id)}{#if i.kind === "approval"}<ApprovalCard item={i} now={p.now} />{:else}<ConnectionCard item={i} now={p.now} onRead={toggle} />{/if}{/each}</div>
  {/if}
{/snippet}

{#snippet group(which: Which, g: Group)}
  {@const shown = which === "unread" ? rowsOf(g) : readRowsOf(g)}
  {@const fresh = which === "unread" ? freshOf(g) : 0}
  {@const rows = shown.filter(i => !decision(i))}
  <InboxGroup group={g} {fresh} open={isOpen(which, g)} now={p.now} id={`inbox-${which}-${g.key}`} onToggle={() => flip(which, g)}
    onMore={g.next ? () => void more(which, g.key) : undefined} onRead={fresh && !busy ? () => void markGroup(g) : undefined}>
    {@render decided(shown.filter(decision))}
    {#if rows.length}<ul class="rows">{#each rows as i (i.id)}<InboxRow item={i} others={listed} now={p.now} grouped onOpen={open} onToggle={toggle} />{/each}</ul>{/if}
  </InboxGroup>
{/snippet}

<div class="inbox">
  <header class="ihead">
    <h2>Inbox</h2>
    <span class="icnt">{inbox.unread.toLocaleString("en-US")} unread</span>
    <span class="sp"></span>
    {#if perm === "default"}<button type="button" onclick={askNotify}>Notify me</button>{/if}
    <button type="button" class="primary" disabled={busy || !markable} onclick={markAll}>Mark all read</button>
  </header>
  {#if perm === "denied"}<p class="quiet">Notifications are blocked for this site. The unread count shows in the tab's title.</p>{/if}
  <AgentStrip agents={strip} now={clock.now} selected={filter.agent ?? null} onPick={h => apply({ ...filter, agent: h ?? undefined })} />

  <div class="find">
    <input type="search" class="isearch" placeholder="Search the inbox" aria-label="Search the inbox" value={search} oninput={e => onSearch(e.currentTarget.value)} />
    <button type="button" class="filters-toggle" aria-expanded={filtersOpen} aria-controls="inbox-filters" onclick={() => { filtersOpen = !filtersOpen; }}>Filters</button>
  </div>
  <div class="filters" id="inbox-filters" data-open={filtersOpen ? "" : undefined}>
    <div class="kinds" role="group" aria-label="Kinds">
      {#each INBOX_KINDS.filter(k => k !== "connection" || owner || filter.kind?.includes(k)) as k (k)}
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
      {@render decided(decisions)}
      {#if unreadGroups.length}<div class="groups">{#each unreadGroups as g (g.key)}{@render group("unread", g)}{/each}</div>{/if}
      {#if !cardItems.length && !unreadGroups.length}
        <p class="note">{filtered ? "No unread items match." : "Nothing unread."}</p>
      {/if}
      {#if unread.next}<button type="button" class="more" onclick={() => moreGroups("unread")}>More pages</button>{/if}
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
            {#if readGroups.length}<div class="groups">{#each readGroups as g (g.key)}{@render group("read", g)}{/each}</div>{/if}
            {#if read.next}<button type="button" class="more" onclick={() => moreGroups("read")}>More pages</button>{/if}
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
    .inbox .dlist { display: grid; gap: 10px; margin-bottom: 10px; min-width: 0; }
    .inbox .rows { margin: 0; padding: 0; }
    .inbox .groups { border-top: 1px solid var(--border); min-width: 0; }
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
