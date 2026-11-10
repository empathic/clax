<svelte:options css="injected" />

<script lang="ts">
  // ClaxBot in the gallery's header (spec 2026-10-21-claxbot §11.4, §11.5),
  // loaded with the gallery's lazy module: the week's meter when ClaxBot is
  // on for this person ("ClaxBot · 310 of 2,000 credits", linking to
  // Settings' Your ClaxBot; the numbers alone at phone width), marked "your
  // key" when their runs use their own model key (credits then pay only for
  // what every key is charged: `meterWords`).
  // It also puts the usage banner at the top of the page (`UsageBanner`,
  // mounted ahead of the page's content, as the gallery's sites are).
  import { mount, onMount, unmount } from "svelte";
  import type { MeBot } from "./api";
  import { myBot } from "./me";
  import { credits, meter, meterWords } from "./model";
  import UsageBanner from "./UsageBanner.svelte";

  let me = $state<MeBot | null>(null);
  const m = $derived(me?.enabled ? meter(me.week) : null);
  const words = $derived(me?.enabled ? meterWords(me) : null);
  const here = location.pathname;
  onMount(() => {
    let view: Record<string, unknown> | null = null;
    let el: HTMLElement | null = null;
    let gone = false;
    void myBot().then(b => {
      if (gone) return;
      me = b;
      const main = document.querySelector("main.gal");
      if (gone || !main || !b?.enabled) return;
      el = document.createElement("div");
      main.prepend(el);
      view = mount(UsageBanner, { target: el, props: { me: b } });
    });
    return () => { gone = true; if (view) void unmount(view); el?.remove(); };
  });
</script>

{#if m && words}
  <a class="bot-meter" class:warn={m.warn} class:out={m.exhausted} href="/settings/claxbot" aria-current={here === "/settings/claxbot" ? "page" : undefined} aria-label={words.label} title={words.title}>
    <span class="word">{words.own ? "ClaxBot · your key ·" : "ClaxBot ·"}</span> {credits(m.spent)}<span class="word">{` of ${credits(m.total)} credits`}</span>
  </a>
{/if}

<style>
  :global {
    .bot-meter { display: inline-flex; align-items: center; gap: 4px; flex: none; min-height: 32px; padding: 0 8px; border-radius: var(--radius-sm); font: 600 13px var(--font); color: var(--fg); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .bot-meter { color: var(--muted); font-weight: 500; }
    .bot-meter.warn { color: var(--tag-yellow-fg); }
    .bot-meter.out { color: var(--danger); }
    .bot-meter:hover, .bot-meter[aria-current="page"] { background: var(--hover); }
    .gbar .sub + .bot-meter { margin-left: auto; }
    @media (pointer: coarse) { .bot-meter { min-height: 44px; } }
    @media (max-width: 860px) { .bot-meter .word { display: none; } }
    /* At phone width beside the meter, the header's other links show their icons alone. */
    @media (max-width: 520px) {
      .gbar:has(.bot-meter) .inbox-link .word, .gbar:has(.bot-meter) .queues-link .word { display: none; }
    }
  }
</style>
