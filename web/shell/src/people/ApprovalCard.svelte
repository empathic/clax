<svelte:options css="injected" />

<script lang="ts">
  // An approval request in its approver's inbox (spec
  // 2026-10-08-multi-agent-multi-person §3.3, §9.3, §9.5): who wants to
  // reach which agent of theirs ("<publisher>'s page, via <viewer>'s
  // browser" for a page's send), how much waits on which page and thread,
  // the withheld content itself (its first 3 lines, then Show all), and the
  // four decisions: this thread, including later replies (only for a
  // request in a thread); always on this page; always everywhere; decline.
  // A decision is the browser's (M20); the card then says what was decided,
  // and keeps the keyboard's focus. A decided or withdrawn request says so,
  // and a gone one says it no longer waits. Every name, title and comment
  // is text. Its styles travel with the inbox page's chunk (injected on
  // mount); state shows in data attributes.
  import type { InboxApproval, InboxItem } from "../api";
  import { relativeTime } from "../format";
  import { approvalOf, heldAnchor, heldCount, personName } from "../q/inbox-model";
  import * as api from "./api";

  type Scope = "thread" | "artifact" | "everywhere" | "decline";
  let { item, now }: { item: InboxItem; now?: Date } = $props();
  // The decision made here, until the item itself says it.
  let made = $state<Pick<InboxApproval, "status" | "scope"> | null>(null);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let all = $state(false);

  const a = $derived(approvalOf(item));
  const id = $derived(a?.id ?? (item.approval as { id: string } | undefined)?.id ?? "");
  const status = $derived(a && a.status !== "open" ? a.status : made?.status ?? a?.status ?? "gone");
  const scope = $derived(a && a.status !== "open" ? a.scope : made?.scope ?? a?.scope ?? null);
  const author = $derived(a ? personName(a.author) : "");
  // Within a sentence, an unnamed owner is "the owner".
  const named = $derived(author === "The owner" ? "the owner" : author);
  const page = $derived(a?.via_page);
  // The content as lines: a comment's first line led by its author.
  type Line = { who?: string; text: string };
  const lines = $derived.by((): Line[] => {
    const c = a?.content;
    if (!c) return [];
    if ("comments" in c) {
      // Every comment is the request's author's (or an agent of theirs).
      return c.comments.flatMap(x => x.body.split(/\r?\n/).map((text, n) => (n ? { text } : { who: x.by_agent ? `${x.author_name ?? "An agent"} (${named}'s agent)` : author, text })));
    }
    const out: Line[] = [];
    if (c.title) out.push({ who: "Title", text: c.title });
    if (c.description) out.push({ who: "Description", text: c.description });
    if (c.files.length) out.push({ who: "Files", text: c.files.join(", ") });
    if (c.docs.length) out.push({ who: "Data", text: c.docs.join(", ") });
    if (c.assets.length) out.push({ who: "Uploads", text: c.assets.map(x => x.content_type).join(", ") });
    return out;
  });
  /** How many lines show before Show all. */
  const LINES = 3;
  // Whether the collapsed text overflows its 3-line clamp (a line that
  // wraps), measured from its laid-out box after each render and again
  // whenever the box changes size, so no clipped text goes without Show all.
  let clipped = $state(false);
  const measure = () => {
    if (all) return;
    const el = document.getElementById(`held-${id}`)?.querySelector<HTMLElement>(".htext");
    clipped = !!el && el.scrollHeight > el.clientHeight + 1;
  };
  $effect(() => {
    void shown;
    measure();
    const el = document.getElementById(`held-${id}`)?.querySelector<HTMLElement>(".htext");
    if (!el) return;
    if (typeof ResizeObserver === "function") {
      const ro = new ResizeObserver(measure);
      ro.observe(el);
      return () => ro.disconnect();
    }
    addEventListener("resize", measure);
    return () => removeEventListener("resize", measure);
  });
  const long = $derived(lines.length > LINES || clipped);
  const shown = $derived(all ? lines : lines.slice(0, LINES));
  const OUTCOME: Record<string, string> = { thread: "Allowed in this thread", artifact: "Allowed on this page", everywhere: "Allowed everywhere" };
  const outcome = $derived(status === "approved" ? `${OUTCOME[scope ?? ""] ?? "Allowed"}` : status === "declined" ? "Declined" : status === "withdrawn" ? "Withdrawn: nothing waits on it any more" : "This request no longer waits for you");
  const threadUrl = $derived(a ? `/a/${encodeURIComponent(a.artifact_id)}${a.thread_id ? `?thread=${encodeURIComponent(a.thread_id)}` : ""}` : null);
  const title = $derived(item.artifact?.title ?? null);
  const anchor = $derived(a ? heldAnchor(item, a) : undefined);

  /** The card's element, found by query: a `bind:this` helper would be split into a chunk the artifact view then loads. */
  const self = () => [...document.querySelectorAll<HTMLElement>(".acard")].find(e => e.dataset.approval === id);
  async function decide(s: Scope) {
    if (busy || !a) return;
    busy = true; err = null;
    try {
      const r = await api.decide(a.id, s);
      made = { status: r.request.status, scope: r.request.scope };
      // Its buttons are gone: the card keeps the keyboard's focus.
      queueMicrotask(() => self()?.focus({ preventScroll: true }));
    } catch (x) { err = x instanceof Error ? x.message : String(x); } finally { busy = false; }
  }
</script>

<article class="acard" data-approval={id} data-status={status} tabindex="-1" aria-label={a ? `${author} wants to reach your agent` : "An approval request"}>
  {#if a}
    <header class="ahead">
      <span><span class="who">{author}</span> wants to reach your {#if a.agent}agent <span class="agent">{a.agent.name}</span>{:else}agents{/if}</span>
      <time datetime={a.created_at}>{relativeTime(a.created_at, now ?? new Date())}</time>
    </header>
    {#if page}<p class="via">{page.authors.map(personName).join(", ")}'s page, via {personName(page.viewer)}'s browser</p>{/if}
    <p class="what">{heldCount(a)} on {#if title}<a href={threadUrl}>{title}</a>{:else}one page{/if}{#if anchor}{` ${anchor}`}{/if}</p>
    {#if lines.length}
      <div class="held" id={`held-${id}`} data-open={all ? "" : undefined}><div class="htext">{#each shown as l, n (n)}{#if l.who}<span class="hwho">{`${l.who}: `}</span>{/if}{l.text}{n < shown.length - 1 ? "\n" : ""}{/each}</div></div>
      {#if long}<button type="button" class="show ghost" aria-expanded={all} aria-controls={`held-${id}`} onclick={() => { all = !all; }}>{all ? "Show less" : "Show all"}</button>{/if}
    {/if}
  {/if}
  {#if status === "open"}
    <div class="acts">
      {#if a?.thread_id}<button type="button" class="primary" disabled={busy} onclick={() => decide("thread")}>Allow this thread, including later replies</button>{/if}
      <button type="button" disabled={busy} onclick={() => decide("artifact")}>Always allow {named} on this page</button>
      <button type="button" disabled={busy} onclick={() => decide("everywhere")}>Always allow {named} everywhere</button>
      <button type="button" class="ghost" disabled={busy} onclick={() => decide("decline")}>Decline</button>
    </div>
  {:else}
    <p class="outcome" role="status">{outcome}</p>
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</article>

<style>
  :global {
    .acard { position: relative; display: grid; gap: 6px; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--agent); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; font-size: 13.5px; }
    .acard:focus { outline: none; }
    .acard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .acard:not([data-status="open"]) { border-left-color: var(--border-strong); }
    .acard p { margin: 0; }
    .acard .ahead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; min-width: 0; font-weight: 600; overflow-wrap: anywhere; }
    .acard .ahead > span { min-width: 0; }
    .acard .agent { color: var(--agent-ink); }
    .acard time { margin-left: auto; font: 400 11.5px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .acard .via, .acard .what { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
    .acard .what a { color: var(--fg); text-decoration: underline; text-decoration-color: var(--border-strong); text-underline-offset: 2px; }
    .acard .what a:hover { text-decoration-color: currentColor; }
    .acard .held { margin: 2px 0 0; padding: 8px 10px; border-radius: var(--radius-sm); background: var(--bg); border: 1px solid var(--border); white-space: pre-wrap; overflow-wrap: anywhere; font: 13px/1.45 var(--font); }
    /* Collapsed, a long line wraps to 3 lines at most too. */
    .acard .held:not([data-open]) .htext { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 3; line-clamp: 3; overflow: hidden; }
    .acard .hwho { font-weight: 600; }
    .acard .show { justify-self: start; min-height: 28px; padding: 2px 8px; font-size: 12.5px; }
    .acard .acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
    .acard .acts button { max-width: 100%; white-space: normal; overflow-wrap: anywhere; text-align: start; }
    .acard .outcome { font-weight: 600; font-size: 13px; }
    .acard .err { color: var(--danger); font-size: 13px; }
    @media (max-width: 520px) { .acard .acts button { flex: 1 1 100%; justify-content: flex-start; } }
  }
</style>
