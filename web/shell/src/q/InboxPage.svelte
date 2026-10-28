<svelte:options css="injected" />

<script lang="ts">
  // `/inbox` (spec 2026-10-06-agent-questions-and-inbox §9.2): everything
  // agents sent back, in one list. The header has the unread count, **Mark
  // all read** and **Notify me** (while the browser has not been asked; a
  // quiet line when notifications are blocked). Search text and filters
  // (Unread, kind, page, agent, dates) are kept in the URL. Open questions,
  // approval requests (spec 2026-10-08-multi-agent-multi-person §9.5) and
  // connection requests (§3.2) are pinned on top as cards under one heading
  // with their count ("Questions" when all are questions, else "Waiting on
  // you"). Below them each page appears once, as a group (`InboxGroup`),
  // newest group first, items about no page in one group, "Other". A group
  // with something new says how many, bold, and starts open on its new
  // items, then its most recent read ones; one with nothing new is a quiet
  // folded line that opens in place. **Show more** pages a group, **Mark
  // read** reads its new items, and **Unread** narrows the list to groups
  // with something new. A group keeps its place and its fold as its items
  // are read, here or elsewhere, until the search changes; under Unread a
  // group read since stays too, shown read, so nothing jumps. Items stay where
  // they are as they change (a row read elsewhere, a card answered): the
  // list is fetched again when the search changes, on a bulk mark, and when
  // the stream resyncs. A row opens its item and marks it read; its dot
  // marks it without opening. `?q=<question>` scrolls to that question's
  // card and focuses it, and `?approval=<request>` to that request's. A
  // decided card stays where it is, saying how; a request decided before
  // shows as a card in its group. A machine that connected with an invite
  // and an agent that ended are rows.
  // Only the owner's browser offers the Connections filter. At phone width
  // the filters sit behind **Filters**. In the owner's browser, where the
  // browser has Web Push, **Notify me on this device** turns on pushes to
  // the device (./push.ts) in place of **Notify me**; on an iPhone outside
  // the Home Screen app a line says how to get them. The agent strip (§13)
  // opens the page, under its header: a chip filters the inbox by its agent; it is
  // fetched again with the list. Tags, beside Page, picks tags (a list with
  // a search, `ui/TagPicker.svelte`): the items about artifacts carrying
  // every one picked.
  import { onMount } from "svelte";
  import { type InboxFilter, type InboxItem, type InboxKind, type StripAgent, type TagRef, getToken, listArtifacts } from "../api";
  import TagPicker from "../ui/TagPicker.svelte";
  import { filterLabel } from "../view/tag-picker-model";
  import { listTags } from "../view/tags-api";
  import { canonical, holds, refsForUrl, same, toggle as toggleTag } from "../view/tags-model";
  import type { EventStream } from "../stream";
  import AgentStrip from "../people/AgentStrip.svelte";
  import { ticker } from "../ui/ticker.svelte";
  import { after } from "../clock";
  import type { InboxFeed, QuestionFeed } from "./feed.svelte";
  import { approvalOf, newestSeq } from "./inbox-model";
  import { INBOX_KINDS, filterFromUrl, filterToUrl } from "./inbox-filter";
  import { GROUP_PAGE, GROUP_ROWS, type Group, MAX_GROUP_PAGE, MORE_ROWS, appendGroups, appendRows, fromWire, groupKey, mergeGroups, takeItem } from "./inbox-groups";
  import ApprovalCard from "../people/ApprovalCard.svelte";
  import ConnectionCard from "../people/ConnectionCard.svelte";
  import InboxGroup from "./InboxGroup.svelte";
  import InboxRow from "./InboxRow.svelte";
  import { inboxAgents } from "./api";
  import { openItem } from "./open";
  import QuestionList from "./QuestionList.svelte";
  import { shared } from "./shared";
  import { type PushEnv, type PushState, pushState, turnOff, turnOn } from "./push";

  type Props = {
    inbox?: InboxFeed;
    questions?: QuestionFeed;
    /** Goes to an item's URL (default: the browser navigates). */
    go?: (url: string) => void;
    /** The page's stream, whose `tags` events refresh the Tags filter's list. */
    stream?: EventStream;
    now?: Date;
    /** What the push controls use of the browser (default: the real one). */
    pushEnv?: PushEnv;
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

  /** The list's groups and the cursor of its next page of groups. */
  type List = { groups: Group[]; next: string | null };
  const initial = filterFromUrl(location.search);
  let filter = $state<InboxFilter>(initial);
  let search = $state(initial.q ?? "");
  const asked = new URLSearchParams(location.search);
  // The card to focus once listed: a question's (`?q=`) or an approval request's (`?approval=`).
  let target: { q: string } | { approval: string } | null = asked.get("q") ? { q: asked.get("q")! } : asked.get("approval") ? { approval: asked.get("approval")! } : null;
  let list = $state<List | null>(null);
  // The cards pinned above the groups: open questions and decisions, and those closed or decided here.
  let cardItems = $state<InboxItem[]>([]);
  // Whether a group is open: as the person set it, else as it was first
  // listed for this search (open with something new, or alone). A group
  // opens when something new comes, and never folds itself: one read here
  // or elsewhere stays where it is until the search changes.
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
  // This device's Web Push, in the owner's browser (null until known, or for anyone else).
  let push = $state<PushState | null>(null);
  let pushBusy = $state(false);
  let pushError = $state<string | null>(null);

  const filtered = $derived([filter.q, filter.kind?.length, filter.artifact, filter.agent, filter.since, filter.until, filter.tag?.length].some(Boolean));
  // Every tag, for the Tags filter, and whether its list is open.
  let allTags = $state<TagRef[]>([]);
  let tagsOpen = $state(false);
  let tagBox: HTMLDivElement | undefined = $state();
  const closeTags = (refocus: boolean) => { tagsOpen = false; if (refocus) tagBox?.querySelector<HTMLButtonElement>(".tbtn")?.focus(); };
  const outside = (e: PointerEvent) => { if (tagsOpen && !tagBox?.contains(e.target as Node)) closeTags(false); };
  const unreadOnly = $derived(filter.read === "unread");
  // A card's view: the question feed's (live), else the item's.
  const qview = (i: InboxItem) => (i.question && (questions.get(i.question.id) ?? i.question))!;
  const cards = $derived(cardItems.filter(i => asCard.includes(i.id)).map(qview).sort((a, b) => a.created_at.localeCompare(b.created_at)));
  /** Whether `i` is a decision's card: an approval request, or a machine's request to connect. */
  const decision = (i: InboxItem) => i.kind === "approval" || (i.kind === "connection" && i.connection?.via !== "invite");
  const decisions = $derived(cardItems.filter(decision).sort((a, b) => a.seq - b.seq));
  // The cards pinned above the groups, under one heading: "Questions" when all are, else "Waiting on you".
  const pinned = $derived(cards.length + decisions.length);
  // What is pinned as a card, so no group lists it as a row.
  const carded = $derived(new Set([...asCard, ...decisions.map(i => i.id)]));
  const rowsOf = (g: Group) => g.items.filter(i => !carded.has(i.id));
  /** How many of a group's items are new: its unread count, less its unread items pinned as cards. */
  const freshOf = (g: Group) => Math.max(0, g.unread - cardItems.filter(i => !i.read && carded.has(i.id) && groupKey(i) === g.key).length);
  // A group whose every item is a pinned card has nothing of its own to show.
  const groups = $derived((list?.groups ?? []).filter(g => rowsOf(g).length || freshOf(g) || g.next));
  const isOpen = (g: Group) => toggled[g.key] ?? listedOpen[g.key] ?? false;
  /** Notes which groups listed start open: one with something new, or the only one. */
  const opening = (gs: Group[], one: boolean) => {
    listedOpen = { ...listedOpen, ...Object.fromEntries(gs.map(g => [g.key, !!listedOpen[g.key] || freshOf(g) > 0 || one])) };
  };
  // The pickers' options, [value, label]: the pages and the harnesses and
  // agents of the items listed, and the one chosen.
  const listed = $derived([...cardItems, ...(list?.groups ?? []).flatMap(g => g.items)]);
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
  const markable = $derived(groups.some(g => freshOf(g) > 0) || cardItems.some(i => !i.read && approvalOf(i)?.status !== "open"));
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
  /** A page of the groups, after `before`: every item's, or the unread ones' with the Unread filter. */
  const fetchGroups = async (before?: string | null, limit?: number): Promise<List> => {
    const r = await inbox.groups(filter, before, GROUP_ROWS, limit);
    return { groups: r.groups.map(fromWire), next: r.next_cursor };
  };

  /** The cards as fetched, `c`, and, with `keep`, those closed or decided
   * here (read now, so no longer listed as unread). */
  const keepCards = (c: InboxItem[], keep: boolean): InboxItem[] => {
    const kept = keep ? [
      ...cards.filter(q => q.status !== "open").map(q => cardItems.find(i => i.question?.id === q.id)!),
      ...decisions.filter(i => i.read),
    ].filter(i => i && !c.some(x => x.id === i.id)) : [];
    return [...kept, ...c];
  };

  /** Fetches the cards and the first page of groups, replacing the list;
   * `keep` keeps the cards closed here, for the same search. */
  async function load(keep = true): Promise<void> {
    const g = ++gen;
    void loadStrip();
    try {
      const [c, l] = await Promise.all([fetchCards(), fetchGroups()]);
      if (g !== gen) return;
      error = null;
      cardItems = keepCards(c, keep);
      list = l;
      opening(l.groups, l.groups.length === 1);
      void fill(g);
      // `?q=` or `?approval=`: that card takes focus, once.
      const t = target;
      if (t) queueMicrotask(() => { if ("q" in t ? toCard(t.q) : toCard(t.approval, "approval")) target = null; });
    } catch (e) {
      if (g === gen) loadFailed(e);
    }
  }

  /** Fetches the cards and as many groups as are listed, and merges them
   * into the list (`mergeGroups`): the groups paged in by More pages, the
   * rows by Show more, and every group's place stay as they are. A mark
   * and a live change refresh; a new search loads. */
  async function refresh(): Promise<void> {
    if (!list) return load();
    const g = ++gen;
    void loadStrip();
    try {
      const shown = list.groups.length;
      const [c, l] = await Promise.all([fetchCards(), fetchGroups(null, Math.min(MAX_GROUP_PAGE, Math.max(GROUP_PAGE, shown)))]);
      if (g !== gen || !list) return;
      error = null;
      cardItems = keepCards(c, true);
      // Under the Unread filter a group read since drops out of the fetch:
      // it stays where it is, all read, until the next load.
      const gone = (x: Group): Group => (unreadOnly ? { ...x, unread: 0, items: x.items.map(i => (i.read ? i : { ...i, read: true })) } : x);
      list = { groups: mergeGroups(list.groups, l.groups, gone), next: list.next ?? l.next };
      opening(l.groups, false);
    } catch (e) {
      if (g === gen) loadFailed(e);
    }
  }

  /** Lists more of each open group whose first rows are all pinned cards. */
  async function fill(g: number): Promise<void> {
    for (const x of groups) if (g === gen && x.next && rowsOf(x).length === 0 && isOpen(x)) await more(x.key);
  }

  /** The next rows of group `key`, appended: its unread ones while there
   * are more (`u<seq>`), then its read ones (`r`, `r<seq>`). */
  async function more(key: string): Promise<void> {
    const at = list?.groups.find(x => x.key === key);
    const cursor = at?.next;
    if (!at || !cursor) return;
    const g = gen;
    const unreadPart = cursor.startsWith("u");
    const before = cursor.slice(1) || null;
    try {
      const r = await inbox.page({ ...filter, read: unreadPart ? "unread" : "read", artifact: key }, before, MORE_ROWS);
      if (g !== gen || !list) return;
      // Past the last unread page, its read ones follow (none with the Unread filter).
      const next = r.next_cursor ? `${unreadPart ? "u" : "r"}${r.next_cursor}` : unreadPart && !unreadOnly && at.total > at.unread ? "r" : null;
      list = { ...list, groups: appendRows(list.groups, key, r.items, next) };
    } catch (e) { loadFailed(e); }
  }

  /** The next page of groups, appended. */
  async function moreGroups(): Promise<void> {
    if (!list?.next) return;
    const g = gen;
    try {
      const r = await fetchGroups(list.next);
      if (g !== gen || !list) return;
      opening(r.groups, false);
      list = { groups: appendGroups(list.groups, r.groups), next: r.next };
    } catch (e) { loadFailed(e); }
  }

  const flip = (g: Group) => { toggled = { ...toggled, [g.key]: !isOpen(g) }; };

  /** Scrolls to question `id`'s card (or approval request `id`'s) and focuses it; false when no card shows it. */
  const toCard = (id: string, kind: "question" | "approval" = "question"): boolean => {
    const el = [...document.querySelectorAll<HTMLElement>(`.inbox [data-${kind}]`)].find(e => e.dataset[kind] === id);
    el?.scrollIntoView?.({ block: "center" });
    el?.focus({ preventScroll: true });
    return !!el;
  };

  /** Applies a new search: into the URL, then the list again, its groups folded or open afresh. */
  function apply(next: InboxFilter): void {
    filter = next;
    history.replaceState(history.state, "", urlOf(next));
    listedOpen = {};
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
  const toggleUnread = () => apply({ ...filter, read: unreadOnly ? undefined : "unread" });
  const set = (k: "artifact" | "agent" | "since" | "until", v: string) => apply({ ...filter, [k]: v || undefined });
  const pickTag = (t: TagRef) => { const tag = toggleTag(filter.tag ?? [], t); apply({ ...filter, tag: tag.length ? tag : undefined }); };
  /** `f`'s address: its tags by name. */
  const urlOf = (f: InboxFilter) => `/inbox${filterToUrl({ ...f, tag: f.tag && refsForUrl(f.tag, allTags) })}`;
  /** Fetches every tag, and holds the Tags filter by tag ID: a renamed tag
   * keeps filtering under its new name, and a deleted one drops out (the
   * list fetched again without it). */
  const loadTags = () => listTags().then(l => {
    allTags = l.tags;
    const was = filter.tag ?? [];
    const tag = canonical(was, allTags, allTags);
    const next = { ...filter, tag: tag.length ? tag : undefined };
    if (tag.length < was.length) apply(next);
    else {
      if (!same(tag, was)) filter = next;
      history.replaceState(history.state, "", urlOf(filter));
    }
  }, () => {});

  /** Marks read the unread items matching `f` up to the newest of `shown`
   * (group `key`'s, or every group's), shows the rows it read as read
   * where they are, then refreshes the list: rows and groups keep their
   * places and folds. */
  async function markUpTo(f: InboxFilter | undefined, shown: InboxItem[], except: string[] = [], key?: string): Promise<void> {
    busy = true;
    try {
      // Up to the newest item shown, so one made since stays unread.
      const upto = newestSeq(shown);
      await inbox.markAll(f, upto, except);
      // An open approval request stays unread until decided.
      const reads = (i: InboxItem) => !i.read && i.seq <= upto && !except.includes(i.id) && approvalOf(i)?.status !== "open";
      if (list) list = { ...list, groups: list.groups.map(g => (key !== undefined && g.key !== key) || !g.items.some(reads) ? g : { ...g, items: g.items.map(i => (reads(i) ? { ...i, read: true } : i)) }) };
      await refresh();
    } catch (e) {
      oops("mark them read")(e);
    }
    busy = false;
  }
  const markAll = () => markUpTo(filtered ? { ...filter } : undefined, listed);
  /** A group's Mark read: the unread items it counts new, up to its newest
   * shown; its page's items pinned above as cards stay unread. */
  const markGroup = (g: Group) => markUpTo({ ...filter, artifact: g.key }, g.items,
    cardItems.filter(i => !i.read && carded.has(i.id) && groupKey(i) === g.key).map(i => i.id), g.key);

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
  /** Turns this device's pushes on or off, from a click (the permission prompt needs one). */
  function setPush(on: boolean): void {
    pushBusy = true;
    pushError = null;
    (on ? turnOn(p.pushEnv) : turnOff(p.pushEnv)).then(
      s => { push = s; perm = N?.permission; },
      e => { pushError = `Could not turn notifications ${on ? "on" : "off"}: ${e instanceof Error ? e.message : String(e)}`; },
    ).finally(() => { pushBusy = false; });
  }

  // An item as it is now: replaced where it shows; for a new unread one the
  // list is fetched again (debounced, so a burst is one fetch).
  let refetchSoon: (() => void) | undefined;
  const soon = () => { refetchSoon?.(); refetchSoon = after(DEBOUNCE_MS, () => { refetchSoon = undefined; void refresh(); }); };
  // An item that changed may change a block: the strip is fetched again (debounced).
  let stripSoon: (() => void) | undefined;
  function take(item: InboxItem): void {
    stripSoon?.();
    stripSoon = after(DEBOUNCE_MS, () => { stripSoon = undefined; void loadStrip(); });
    // An item shown that was read (a decision, a mark elsewhere), or unread
    // again, moves its group's count with it; it stays where it is.
    const was = cardItems.find(x => x.id === item.id) ?? list?.groups.flatMap(g => g.items).find(x => x.id === item.id);
    const d = was && was.read !== item.read ? (item.read ? -1 : 1) : 0;
    if (cardItems.some(x => x.id === item.id)) cardItems = cardItems.map(x => (x.id === item.id ? item : x));
    if (list) list = { ...list, groups: takeItem(list.groups, item, d) };
    // One not shown: a new unread item, or one of a shown group's past its
    // listed rows, read or unread elsewhere, whose change in its group's
    // count is unknown here: the list is fetched again.
    if (!was && (!item.read || list?.groups.some(g => g.key === groupKey(item)))) soon();
  }

  onMount(() => {
    document.title = "Inbox · Clax";
    void getToken().then(t => {
      owner = !!t;
      if (owner) void pushState(p.pushEnv).then(s => { push = s; }, () => { push = "unsupported"; });
    });
    void listArtifacts().then(a => { artifacts = a.map(x => [x.id, x.title]); }, () => {});
    void loadTags();
    // The tags themselves changed (made, renamed, recoloured, deleted).
    const offTags = p.stream?.watch(["gallery"], e => { if (e.type === "tags" && e.artifact_id == null) void loadTags(); });
    // Fetched once on open: now, or when the inbox topic first goes live.
    const off = inbox.listen(c => { if ("item" in c) take(c.item); else if (list) soon(); else void load(); });
    if (!inbox.waiting) void load();
    return () => { off(); offTags?.(); typing?.(); refetchSoon?.(); stripSoon?.(); };
  });
</script>

{#snippet decided(list: InboxItem[])}
  {#if list.length}
    <div class="dlist">{#each list as i (i.id)}{#if i.kind === "approval"}<ApprovalCard item={i} now={p.now} />{:else}<ConnectionCard item={i} now={p.now} onRead={toggle} />{/if}{/each}</div>
  {/if}
{/snippet}

{#snippet group(g: Group)}
  {@const shown = rowsOf(g)}
  {@const fresh = freshOf(g)}
  {@const rows = shown.filter(i => !decision(i))}
  <InboxGroup group={g} {fresh} open={isOpen(g)} now={p.now} id={`inbox-group-${g.key}`} onToggle={() => flip(g)}
    onMore={g.next ? () => void more(g.key) : undefined} onRead={fresh && !busy ? () => void markGroup(g) : undefined}>
    {@render decided(shown.filter(decision))}
    {#if rows.length}<ul class="rows">{#each rows as i (i.id)}<InboxRow item={i} others={listed} now={p.now} grouped onOpen={open} onToggle={toggle} />{/each}</ul>{/if}
  </InboxGroup>
{/snippet}

<svelte:window onpointerdown={outside} />

<div class="inbox">
  <header class="ihead">
    <h2>Inbox</h2>
    <span class="icnt">{inbox.unread.toLocaleString("en-US")} unread</span>
    <span class="sp"></span>
    {#if push === "off"}<button type="button" class="pushon" disabled={pushBusy} onclick={() => setPush(true)}>Notify me on this device</button>
    {:else if push === "on"}<span class="icnt">Notifications on</span><button type="button" disabled={pushBusy} onclick={() => setPush(false)}>Turn off notifications</button>
    {:else if perm === "default" && push !== "install"}<button type="button" onclick={askNotify}>Notify me</button>{/if}
    <button type="button" class="primary" disabled={busy || !markable} onclick={markAll}>Mark all read</button>
  </header>
  {#if perm === "denied"}<p class="quiet">Notifications are blocked for this site. The unread count shows in the tab's title.</p>{/if}
  {#if push === "install"}<p class="quiet">To get notifications on this device, add Clax to the Home Screen (Share, then Add to Home Screen) and open it from there.</p>{/if}
  {#if pushError}<p class="err" role="alert">{pushError}</p>{/if}
  <AgentStrip agents={strip} now={clock.now} selected={filter.agent ?? null} onPick={h => apply({ ...filter, agent: h ?? undefined })} />

  <div class="find">
    <input type="search" class="isearch" placeholder="Search the inbox" aria-label="Search the inbox" value={search} oninput={e => onSearch(e.currentTarget.value)} />
    <button type="button" class="filters-toggle" aria-expanded={filtersOpen} aria-controls="inbox-filters" onclick={() => { filtersOpen = !filtersOpen; }}>Filters</button>
  </div>
  <div class="filters" id="inbox-filters" data-open={filtersOpen ? "" : undefined}>
    <button type="button" class="kind unread-chip" aria-pressed={unreadOnly} onclick={toggleUnread}>Unread</button>
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
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="pick tagpick" bind:this={tagBox} onfocusout={e => { if (tagsOpen && !tagBox?.contains(e.relatedTarget as Node | null) && e.relatedTarget) closeTags(false); }}>
      <span id="inbox-tags-l">Tags</span>
      <button type="button" class="tbtn" aria-haspopup="dialog" aria-expanded={tagsOpen} aria-labelledby="inbox-tags-l inbox-tags-v" onclick={() => { tagsOpen = !tagsOpen; }}><span id="inbox-tags-v">{filterLabel(refsForUrl(filter.tag ?? [], allTags))}</span></button>
      {#if tagsOpen}
        <div class="ipop" role="dialog" aria-label="Filter by tags">
          <TagPicker tags={allTags} chosen={t => holds(filter.tag ?? [], t)} onToggle={pickTag} label="Tags the items' pages carry, all of them" onClose={() => closeTags(true)}>
            {#snippet empty()}<p class="tn">There are no tags yet.</p>{/snippet}
            {#snippet foot()}{#if filter.tag?.length}<button type="button" class="ghost iclear" onclick={() => { apply({ ...filter, tag: undefined }); closeTags(true); }}>Any tag</button>{/if}{/snippet}
          </TagPicker>
        </div>
      {/if}
    </div>
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

  <section class="sec" aria-label="Items">
    {#if !list}
      {#if !error}<p class="note">Loading…</p>{/if}
    {:else}
      {#if pinned}<h3 class="pinned-h" id="inbox-pinned-h">{decisions.length ? "Waiting on you" : "Questions"}<span class="c">{fmt(pinned)}</span></h3>{/if}
      <QuestionList list={cards} feed={questions} now={p.now} />
      {@render decided(decisions)}
      {#if groups.length}<div class="groups">{#each groups as g (g.key)}{@render group(g)}{/each}</div>{/if}
      {#if !cardItems.length && !groups.length}
        <p class="note">{unreadOnly ? (filtered ? "No unread items match." : "Nothing unread.") : filtered ? "No items match." : "Nothing in the inbox yet."}</p>
      {/if}
      {#if list.next}<button type="button" class="more" onclick={moreGroups}>More pages</button>{/if}
    {/if}
  </section>
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
    .inbox .tagpick { position: relative; }
    .inbox .tagpick .tbtn { min-height: 32px; max-width: 220px; justify-content: flex-start; border-color: var(--border-strong); overflow: hidden; text-overflow: ellipsis; }
    .inbox .tagpick .ipop { position: absolute; top: calc(100% + 6px); left: 0; z-index: 20; width: 280px; max-width: calc(100vw - 32px); padding: 10px; background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
    .inbox .tagpick .iclear { align-self: flex-start; min-height: 28px; padding: 2px 8px; font-size: 12.5px; color: var(--muted); }
    .inbox .err { margin: 12px 0 0; color: var(--danger); font-size: 13px; }
    .inbox .sec { margin-top: 22px; min-width: 0; }
    .inbox .pinned-h .c { font: 400 12.5px var(--mono); color: var(--muted); font-variant-numeric: tabular-nums; }
    .inbox .pinned-h { display: flex; align-items: baseline; gap: 8px; margin: 0 0 8px; font: 600 13px/1.4 var(--font); color: var(--muted); }
    .inbox .sec .qlist { margin-bottom: 10px; }
    .inbox .dlist { display: grid; gap: 10px; margin-bottom: 10px; min-width: 0; }
    .inbox .rows { margin: 0; padding: 0; }
    .inbox .groups { border-top: 1px solid var(--border); min-width: 0; }
    .inbox .note { margin: 8px 0; color: var(--muted); font-size: 13px; }
    .inbox .more { margin-top: 10px; }
    @media (max-width: 700px) {
      .inbox .filters-toggle { display: inline-flex; }
      .inbox .filters { flex-direction: column; align-items: stretch; }
      .inbox .filters:not([data-open]) { display: none; }
      .inbox .pick { justify-content: space-between; }
      .inbox .pick select, .inbox .pick input, .inbox .tagpick .tbtn { flex: 1; max-width: none; min-width: 0; }
      .inbox .tagpick .ipop { left: auto; right: 0; }
    }
  }
</style>
