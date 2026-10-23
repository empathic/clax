<svelte:options css="injected" />

<script lang="ts">
  // The usage banner (spec 2026-10-21-claxbot §7.4, §11.4): from 80 % of the
  // week's credits a warning that ClaxBot keeps working until the limit;
  // at the limit, that ClaxBot is paused until the reset (pages, comments
  // and local agents keep working), with how to ask an admin for a lift.
  // An unenforced person sees the numbers, never the pause.
  import type { MeBot } from "./api";
  import { banner } from "./model";

  let { me }: { me: MeBot | null } = $props();
  const b = $derived(banner(me));
  let asking = $state(false);
</script>

{#if b}
  <div class="bbanner" class:out={b.kind === "exhausted"} role={b.kind === "exhausted" ? "alert" : "status"}>
    <span class="ic" aria-hidden="true">{b.kind === "exhausted" ? "⛔" : "⚠︎"}</span>
    <p><b>{b.text}</b> {b.detail}</p>
    {#if b.kind === "exhausted"}
      <button type="button" class="ghost" aria-expanded={asking} aria-controls="bbanner-ask" onclick={() => { asking = !asking; }}>Ask an admin for a lift</button>
      {#if asking}<p id="bbanner-ask" class="ask">An admin can raise your weekly credits under Admin › People &amp; flags. Tell one what you need ClaxBot for this week; <a href="/me">You</a> shows what you have used.</p>{/if}
    {/if}
  </div>
{/if}

<style>
  :global {
    .bbanner { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; margin: 0 0 18px; padding: 10px 14px; border-radius: var(--radius-sm); background: var(--tag-yellow-bg); color: var(--fg); border: 1px solid var(--tag-yellow-bd); font-size: 13.5px; }
    .bbanner.out { background: var(--danger-tint); border-color: var(--danger); }
    .bbanner p { margin: 0; flex: 1 1 280px; min-width: 0; line-height: 1.5; }
    .bbanner .ic { flex: none; }
    .bbanner button { white-space: normal; text-align: start; }
    .bbanner .ask { flex-basis: 100%; color: var(--fg); }
    .bbanner a { text-decoration: underline; }
  }
</style>
