<script lang="ts">
  // The gallery header's links, in the slot before the search box and the
  // theme switch (theme.css `.glinks`), each decided at the header's first
  // render (`view/gallery-start.ts` `header`), so none arrives later to
  // move another: ClaxBot's meter (`BotMeter`, from `bot`, the header's
  // `GET /api/me/bot` answer, when ClaxBot is on for this person), **Inbox**
  // with its unread count for a browser with an inbox (spec
  // 2026-10-06-agent-questions-and-inbox §9.4), **Queues** for the owner and
  // the instance's admins, the feedback menu's place for a person who may
  // report (`Menu` fills it), and Settings' gear for every member (spec
  // 2026-10-08-multi-agent-multi-person §17). While the gear shows, the
  // Settings key (Cmd+K, or Ctrl+K) opens Settings' search over the page
  // (`settings/palette.ts`, loaded then). Props are read off `p`.
  import type { Component } from "svelte";
  import type { MeBot } from "../bot/api";
  import BotMeter from "./BotMeter.svelte";

  const p: { current?: string; inbox: boolean; unread: number; queues: boolean; feedback: boolean; member: boolean; bot?: MeBot | null; Menu?: Component | null } = $props();
  const key = (e: KeyboardEvent) => {
    if (!p.member || !((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "k")) return;
    e.preventDefault();
    void import("../settings/palette").then(m => m.openPalette(), () => {});
  };
</script>

<svelte:window onkeydown={key} />

<span class="glinks">
  {#if p.bot?.enabled}<BotMeter me={p.bot} />{/if}
  {#if p.inbox}
    <a class="inbox-link" href="/inbox" aria-current={p.current === "inbox" ? "page" : undefined} aria-label={p.unread ? `Inbox, ${p.unread} unread` : "Inbox"}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 9.5l1.6-6h7.8l1.6 6v3.5h-11z" /><path d="M2.5 9.5h3.2l.8 1.5h3l.8-1.5h3.2" /></svg>
      <span class="word">Inbox</span>{#if p.unread}<span class="count">{p.unread}</span>{/if}
    </a>
  {/if}
  {#if p.queues}
    <a class="queues-link" href="/queues" aria-label="Queues" aria-current={p.current === "queues" ? "page" : undefined}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 4h11M2.5 8h11M2.5 12h7" /></svg>
      <span class="word">Queues</span>
    </a>
    <a class="queues-link" href="/reports" aria-label="Reports" aria-current={p.current === "reports" ? "page" : undefined}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 2.5h6l2.5 2.5v8.5h-8.5zM6 7.5h4.5M6 10h4.5" /></svg>
      <span class="word">Reports</span>
    </a>
  {/if}
  {#if p.feedback}<span class="fb-slot">{#if p.Menu}<p.Menu />{/if}</span>{/if}
  {#if p.member}
    <a class="settings-link" href="/settings" aria-label="Settings" title="Settings" aria-current={p.current === "settings" ? "page" : undefined}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6.7 3.1L6.9 1.4L9.1 1.4L9.3 3.1L10.6 3.6L11.9 2.5L13.5 4.1L12.4 5.4L12.9 6.7L14.6 6.9L14.6 9.1L12.9 9.3L12.4 10.6L13.5 11.9L11.9 13.5L10.6 12.4L9.3 12.9L9.1 14.6L6.9 14.6L6.7 12.9L5.4 12.4L4.1 13.5L2.5 11.9L3.6 10.6L3.1 9.3L1.4 9.1L1.4 6.9L3.1 6.7L3.6 5.4L2.5 4.1L4.1 2.5L5.4 3.6Z" /><circle cx="8" cy="8" r="2.1" /></svg>
    </a>
  {/if}
</span>
