<svelte:options css="injected" />

<script lang="ts">
  // One inbox item as a row (spec 2026-10-06-agent-questions-and-inbox
  // §9.2): its kind's icon, who did what on which page, its text on one line
  // ("(deleted)" when its source is gone), its age, and the read dot. The row
  // is a button that opens the item; the dot is its own button, which marks
  // the item read or unread without opening it. Every string is text. Its
  // styles travel with the question module's lazy chunk (injected on mount).
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
  };
  const p: Props = $props();
  const title = $derived(itemTitle(p.item, p.others ?? []));
  const text = $derived(p.item.gone ? "(deleted)" : itemText(p.item));
  const KIND_NAMES = { reply: "Reply", version: "Version", published: "Published", question: "Question", finished: "Finished" } as const;
</script>

<li class="irow" class:unread={!p.item.read} data-item={p.item.id}>
  <button type="button" class="open" onclick={() => p.onOpen(p.item)}>
    <span class="ico" title={KIND_NAMES[p.item.kind]}>
      <svg viewBox="0 0 16 16" aria-hidden="true">
        {#if p.item.kind === "reply"}<path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z" />
        {:else if p.item.kind === "version"}<path d="M8 2.5l5.5 3-5.5 3-5.5-3z" /><path d="M2.5 8.5l5.5 3 5.5-3" />
        {:else if p.item.kind === "published"}<path d="M4 2.5h5l3 3v8H4z" /><path d="M8 7v4.5M5.75 9.25h4.5" />
        {:else if p.item.kind === "question"}<circle cx="8" cy="8" r="5.5" /><path d="M6.4 6.4a1.7 1.7 0 1 1 2.3 1.6c-.5.2-.7.6-.7 1.1v.4" /><path d="M8 11.4v.1" />
        {:else}<circle cx="8" cy="8" r="5.5" /><path d="M5.5 8.2l1.8 1.8 3.3-3.6" />{/if}
      </svg>
      <span class="sr">{KIND_NAMES[p.item.kind]}:</span>
    </span>
    <span class="main">
      <span class="title">{title}</span>
      {#if text}<span class="text" class:gone={p.item.gone}>{text}</span>{/if}
    </span>
    <time datetime={p.item.created_at}>{relativeTime(p.item.created_at, p.now ?? new Date())}</time>
  </button>
  <button type="button" class="dot" aria-label={p.item.read ? "Mark unread" : "Mark read"} title={p.item.read ? "Mark unread" : "Mark read"}
    onclick={e => { e.stopPropagation(); p.onToggle(p.item); }}><span aria-hidden="true"></span></button>
</li>

<style>
  .irow { position: relative; display: flex; align-items: stretch; list-style: none; border-bottom: 1px solid var(--border); min-width: 0; }
  .open { flex: 1; min-width: 0; display: grid; grid-template-columns: auto minmax(0, 1fr) auto; align-items: start; gap: 2px 10px; padding: 9px 4px 9px 8px; background: none; border: 0; border-radius: var(--radius-sm); text-align: left; white-space: normal; font: 400 13.5px/1.4 var(--font); color: var(--fg); }
  .open:not(:disabled):hover { background-color: var(--hover); }
  .ico { display: inline-grid; place-items: center; width: 22px; height: 22px; color: var(--muted); }
  .ico svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linecap: round; stroke-linejoin: round; }
  .unread .ico { color: var(--agent-ink); }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .main { display: grid; gap: 1px; min-width: 0; }
  .title { overflow-wrap: anywhere; color: var(--muted); }
  .unread .title { color: var(--fg); font-weight: 600; }
  .text { color: var(--muted); font-size: 12.5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .text.gone { font-style: italic; }
  time { font: 400 11.5px/19px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
  .dot { flex: none; align-self: center; width: 32px; min-height: 32px; padding: 0; background: none; border-color: transparent; border-radius: 50%; }
  .dot:not(:disabled):hover { background-color: var(--hover); border-color: transparent; }
  .dot span { width: 9px; height: 9px; border-radius: 50%; box-shadow: inset 0 0 0 1.5px var(--border-strong); }
  .unread .dot span { background: var(--you); box-shadow: none; }
  @media (pointer: coarse) { .dot { width: 44px; min-height: 44px; } }
</style>
