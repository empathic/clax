<svelte:options css="injected" />

<script lang="ts">
  // **Inbox** with the unread count (spec 2026-10-06-agent-questions-and-inbox
  // §9.4), linking to `/inbox`, in the gallery's header and the artifact
  // view's top bar; the count is hidden at 0. Shown to the owner only. At
  // phone width the word gives way to the icon, and in the artifact view's
  // top bar the count to a dot on it (the label still says the count). Its styles also carry the
  // dot the artifact view's threads controls show while a question about
  // the artifact is open (`<html data-questions>`, `wire.svelte.ts`).
  import type { InboxFeed } from "./feed.svelte";
  import { shared } from "./shared";

  const p: { feed?: InboxFeed; current?: boolean } = $props();
  const f = $derived(p.feed ?? shared().inbox);
</script>

{#if f.owner}
  <a class="inbox-link" href="/inbox" aria-current={p.current ? "page" : undefined}
    aria-label={f.unread ? `Inbox, ${f.unread} unread` : "Inbox"}>
    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 9.5l1.6-6h7.8l1.6 6v3.5h-11z" /><path d="M2.5 9.5h3.2l.8 1.5h3l.8-1.5h3.2" /></svg>
    <span class="word">Inbox</span>{#if f.unread}<span class="count">{f.unread}</span>{/if}
  </a>
{/if}

<style>
  :global {
    .inbox-link { display: inline-flex; align-items: center; gap: 6px; flex: none; min-height: 32px; padding: 0 8px; border-radius: var(--radius-sm); font: 600 14px var(--font); color: var(--fg); text-decoration: none; white-space: nowrap; }
    .inbox-link:hover, .inbox-link[aria-current="page"] { background: var(--hover); }
    .inbox-link svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linejoin: round; color: var(--muted); }
    .inbox-link .count { min-width: 20px; padding: 0 6px; border-radius: 999px; background: var(--you); color: var(--on-you); font: 600 11.5px/20px var(--font); text-align: center; font-variant-numeric: tabular-nums; }
    @media (max-width: 700px) { .topbar .inbox-link { position: relative; } .topbar .inbox-link .count { position: absolute; top: 5px; right: 1px; min-width: 9px; height: 9px; padding: 0; font-size: 0; } .inbox-link .word { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; } .inbox-link { padding: 0 6px; } }
    @media (pointer: coarse) { .inbox-link { min-height: 44px; } }
    html[data-questions] .topbar button.threads::after, html[data-questions] .phone-tabs button:last-child::after {
      content: "" / ", a question for you"; display: inline-block; width: 7px; height: 7px; margin-left: 6px; border-radius: 50%; background: var(--you); vertical-align: 1px;
    }
  }
</style>
