<svelte:options css="injected" />

<script lang="ts">
  // One inbox item as a row (spec 2026-10-06-agent-questions-and-inbox
  // §9.2): its kind's icon, who did what on which page, its text on one line
  // ("(deleted)" when its source is gone), its age, and the read dot. The row
  // is a button that opens the item; the dot is its own button, which marks
  // the item read or unread without opening it. An ended agent's row lists
  // the threads its end released as links below it, to send them again;
  // its text wraps rather than cut what was released. Every string is text. Its
  // styles travel with the question module's lazy chunk (injected on mount);
  // its state shows in data attributes (see `QuestionCard`).
  import type { InboxItem } from "../api";
  import { relativeTime } from "../format";
  import { itemText, itemTitle } from "./inbox-model";

  type Props = {
    item: InboxItem;
    /** The items listed beside it: an agent is named by harness alone unless another of its harness is among them. */
    others?: InboxItem[];
    now?: Date;
    onOpen(i: InboxItem): void;
    onToggle(i: InboxItem): void;
    /** Where opening it leads, below its text (the extension's panel: an approval request is decided in the shell's inbox). */
    hint?: string;
  };
  const p: Props = $props();
  const title = $derived(itemTitle(p.item, p.others ?? []));
  const text = $derived(p.item.gone ? "(deleted)" : itemText(p.item));
  const mark = $derived(p.item.read ? "Mark unread" : "Mark read");
  const released = $derived(p.item.kind === "ended" ? p.item.ended?.threads ?? [] : []);
  const KIND_NAMES = { reply: "Reply", version: "Version", published: "Published", question: "Question", finished: "Finished", approval: "Approval", connection: "Connection", ended: "Ended" } as const;
  // One path per kind (an SVG fragment would need the runtime's SVG templates).
  const ICONS = {
    reply: "M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z",
    version: "M8 2.5l5.5 3-5.5 3-5.5-3zM2.5 8.5l5.5 3 5.5-3",
    published: "M4 2.5h5l3 3v8H4zM8 7v4.5M5.75 9.25h4.5",
    question: "M2.5 8a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0-11 0M6.4 6.4a1.7 1.7 0 1 1 2.3 1.6c-.5.2-.7.6-.7 1.1v.4M8 11.4v.1",
    finished: "M2.5 8a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0-11 0M5.5 8.2l1.8 1.8 3.3-3.6",
    approval: "M8 2.5l5 2v3.5c0 3-2.2 5-5 5.5c-2.8-.5-5-2.5-5-5.5V4.5z",
    connection: "M3 5.5h10v6H3zM6 13.5h4M8 11.5v2",
    ended: "M2.5 8a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0-11 0M5.5 8h5",
  } as const;
</script>

<li class="irow" data-unread={p.item.read ? undefined : ""} data-item={p.item.id}>
  <button type="button" class="open" onclick={() => p.onOpen(p.item)}>
    <span class="ico">
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d={ICONS[p.item.kind]} /></svg>
      <span class="sr">{KIND_NAMES[p.item.kind]}:</span>
    </span>
    <span class="main">
      <span class="title" id={`irow-${p.item.id}`}>{title}</span>
      {#if text}<span class="text" data-gone={p.item.gone ? "" : undefined} data-wrap={p.item.kind === "ended" ? "" : undefined}>{text}</span>{/if}
      {#if p.hint}<span class="hint">{p.hint}</span>{/if}
    </span>
    <time datetime={p.item.created_at}>{relativeTime(p.item.created_at, p.now ?? new Date())}</time>
  </button>
  <button type="button" class="dot" aria-describedby={`irow-${p.item.id}`} aria-label={mark} title={mark}
    onclick={() => p.onToggle(p.item)}><span aria-hidden="true"></span></button>
  {#if released.length}
    <ul class="released" aria-label="Released threads">
      {#each released as t (t.id)}<li><a href={t.url}>{t.title ?? "a deleted page"}</a> <span class="tid">· thread {t.id.slice(-6)}</span></li>{/each}
    </ul>
  {/if}
</li>

<style>
  :global {
    .irow { position: relative; display: flex; align-items: stretch; list-style: none; border-bottom: 1px solid var(--border); min-width: 0; }
    .irow .open { flex: 1; min-width: 0; display: grid; grid-template-columns: auto minmax(0, 1fr) auto; align-items: start; gap: 2px 10px; padding: 9px 4px 9px 8px; background: none; border: 0; border-radius: var(--radius-sm); text-align: left; white-space: normal; font: 400 13.5px/1.4 var(--font); color: var(--fg); }
    .irow .open:not(:disabled):hover { background-color: var(--hover); }
    .irow .ico { display: inline-grid; place-items: center; width: 22px; height: 22px; color: var(--muted); }
    .irow .ico svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linecap: round; stroke-linejoin: round; }
    .irow[data-unread] .ico { color: var(--agent-ink); }
    .irow .main { display: grid; gap: 1px; min-width: 0; }
    .irow .title { overflow-wrap: anywhere; color: var(--muted); }
    .irow[data-unread] .title { color: var(--fg); font-weight: 600; }
    .irow .text { color: var(--muted); font-size: 12.5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .irow .text[data-gone] { font-style: italic; }
    .irow .text[data-wrap] { white-space: normal; overflow-wrap: anywhere; }
    .irow .hint { color: var(--agent-ink); font-size: 12.5px; font-weight: 500; }
    .irow { flex-wrap: wrap; }
    .irow .released { flex-basis: 100%; margin: -4px 0 8px; padding: 0 40px 0 40px; display: grid; gap: 2px; font: 400 12.5px/1.4 var(--font); }
    .irow .released li { list-style: none; min-width: 0; overflow-wrap: anywhere; color: var(--muted); }
    .irow .released a { color: var(--fg); text-decoration: underline; text-decoration-color: var(--border-strong); text-underline-offset: 2px; }
    .irow .released a:hover { text-decoration-color: currentColor; }
    .irow .released .tid { font-variant-numeric: tabular-nums; }
    .irow time { font: 400 11.5px/19px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .irow .dot { flex: none; align-self: center; width: 32px; min-height: 32px; padding: 0; background: none; border-color: transparent; border-radius: 50%; }
    .irow .dot:not(:disabled):hover { background-color: var(--hover); border-color: transparent; }
    .irow .dot span { width: 9px; height: 9px; border-radius: 50%; box-shadow: inset 0 0 0 1.5px var(--border-strong); }
    .irow[data-unread] .dot span { background: var(--you); box-shadow: none; }
    @media (pointer: coarse) { .irow .dot { width: 44px; min-height: 44px; } }
  }
</style>
