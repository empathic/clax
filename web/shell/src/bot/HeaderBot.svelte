<svelte:options css="injected" />

<script lang="ts">
  // ClaxBot in the gallery's header (spec 2026-10-21-claxbot §11.4, §11.5),
  // loaded with the gallery's lazy module: the week's meter when ClaxBot is
  // on for this person ("ClaxBot · 310 of 2,000 credits", linking to `/me`;
  // the numbers alone at phone width), and Admin for the owner and admins.
  // It also puts the usage banner at the top of the page (`UsageBanner`,
  // mounted ahead of the page's content, as the gallery's sites are).
  import { mount, onMount, unmount } from "svelte";
  import type { MeBot, Role } from "./api";
  import { isAdmin, myBot, myRole } from "./me";
  import { credits, meter } from "./model";
  import UsageBanner from "./UsageBanner.svelte";

  let me = $state<MeBot | null>(null);
  let role = $state<Role | null>(null);
  const m = $derived(me?.enabled ? meter(me.week) : null);
  const here = location.pathname;
  onMount(() => {
    let view: Record<string, unknown> | null = null;
    let el: HTMLElement | null = null;
    let gone = false;
    void myBot().then(async b => {
      if (gone) return;
      me = b;
      role = await myRole(b);
      const main = document.querySelector("main.gal");
      if (gone || !main || !b?.enabled) return;
      el = document.createElement("div");
      main.prepend(el);
      view = mount(UsageBanner, { target: el, props: { me: b } });
    });
    return () => { gone = true; if (view) void unmount(view); el?.remove(); };
  });
</script>

{#if m}
  <a class="bot-meter" class:warn={m.warn} class:out={m.exhausted} href="/me" aria-current={here === "/me" ? "page" : undefined} aria-label={`ClaxBot: ${credits(m.spent)} of ${credits(m.total)} credits used this week`}>
    <span class="word">ClaxBot ·</span> {credits(m.spent)}<span class="word">{` of ${credits(m.total)} credits`}</span>
  </a>
{/if}
{#if isAdmin(role)}<a class="admin-link" href="/admin" aria-current={here === "/admin" ? "page" : undefined}>Admin</a>{/if}

<style>
  :global {
    .bot-meter, .admin-link { display: inline-flex; align-items: center; gap: 4px; flex: none; min-height: 32px; padding: 0 8px; border-radius: var(--radius-sm); font: 600 13px var(--font); color: var(--fg); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .bot-meter { color: var(--muted); font-weight: 500; }
    .bot-meter.warn { color: var(--tag-yellow-fg); }
    .bot-meter.out { color: var(--danger); }
    .bot-meter:hover, .admin-link:hover, .bot-meter[aria-current="page"], .admin-link[aria-current="page"] { background: var(--hover); }
    .gbar .sub + .bot-meter, .gbar .sub + .admin-link { margin-left: auto; }
    @media (pointer: coarse) { .bot-meter, .admin-link { min-height: 44px; } }
    @media (max-width: 860px) { .bot-meter .word { display: none; } }
    /* At phone width Admin gives way (People links to it), and beside the
       meter the header's other links show their icons alone. */
    @media (max-width: 520px) {
      .gbar .admin-link { display: none; }
      .gbar:has(.bot-meter) .inbox-link .word, .gbar:has(.bot-meter) .people-link .word, .gbar:has(.bot-meter) .queues-link .word { display: none; }
    }
  }
</style>
