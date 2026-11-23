<svelte:options css="injected" />

<script lang="ts">
  // **Inbox** with the unread count (spec 2026-10-06-agent-questions-and-inbox
  // §9.4), linking to `/inbox`: the gallery's header's (`q/InboxLink.svelte`,
  // from the inbox feed) and the artifact view's top bar's and phone tab
  // bar's (from what the page knows, `view/deciders.svelte.ts`, until the
  // feed takes over). The count is hidden at 0; drawn before it is known
  // (`unread` null), the link keeps its badge's place, empty, so the count
  // fills it without moving the link, and keeps it at 0. At phone width the artifact view's moves from
  // the top bar to the phone's tab bar. Its styles also carry the dot the
  // artifact view's threads controls show while a question about the
  // artifact is open (`<html data-questions>`, `q/wire.ts`), whose place
  // they keep in a browser with an inbox (`<html data-inbox>`,
  // `view/deciders.svelte.ts`).
  let { unread, current = false }: { unread: number | null; current?: boolean } = $props();
  // svelte-ignore state_referenced_locally
  const reserved = unread === null;
</script>

<a class="inbox-link" href="/inbox" aria-current={current ? "page" : undefined}
  aria-label={unread ? `Inbox, ${unread} unread` : "Inbox"}>
  <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 9.5l1.6-6h7.8l1.6 6v3.5h-11z" /><path d="M2.5 9.5h3.2l.8 1.5h3l.8-1.5h3.2" /></svg>
  <span class="word">Inbox</span>{#if unread}<span class="count">{unread}</span>{:else if reserved}<span class="count-hold" aria-hidden="true">0</span>{/if}
</a>

<style>
  :global {
    .inbox-link { display: inline-flex; align-items: center; gap: 6px; flex: none; min-height: 32px; padding: 0 8px; border-radius: var(--radius-sm); font: 600 14px var(--font); color: var(--fg); text-decoration: none; white-space: nowrap; }
    .inbox-link:hover, .inbox-link[aria-current="page"] { background: var(--hover); }
    .inbox-link svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linejoin: round; color: var(--muted); }
    .inbox-link .count-hold { visibility: hidden; }
    .inbox-link .count, .inbox-link .count-hold { min-width: 20px; padding: 0 6px; border-radius: 999px; background: var(--you); color: var(--on-you); font: 600 11.5px/20px var(--font); text-align: center; font-variant-numeric: tabular-nums; }
    @media (max-width: 700px) { .topbar .inbox-link { display: none; } }
    .phone-tabs .inbox-link { flex: 1; justify-content: center; min-height: 52px; border-radius: 0; font: 400 15px var(--font); color: var(--muted); }
    @media (pointer: coarse) { .inbox-link { min-height: 44px; } }
    /* In a browser with an inbox (`html[data-inbox]`) the dot's place is
       kept from the first render, so the dot moves nothing when it shows. */
    html:is([data-inbox], [data-questions]) .topbar button.threads::after, html:is([data-inbox], [data-questions]) .phone-tabs button:last-of-type::after {
      content: "" / ", a question for you"; display: inline-block; width: 7px; height: 7px; margin-left: 6px; border-radius: 50%; background: var(--you); vertical-align: 1px; visibility: hidden;
    }
    html[data-questions] .topbar button.threads::after, html[data-questions] .phone-tabs button:last-of-type::after { visibility: inherit; }
  }
</style>
