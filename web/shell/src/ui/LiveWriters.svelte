<svelte:options css="injected" />

<script lang="ts" module>
  /** Someone writing, named as the view knows them, in their live colour. */
  export type LiveWriter = { person_id: string; name: string; color: number; text: string; typing: boolean };
</script>

<script lang="ts">
  // Other people writing in a thread or a new comment (the live layers, spec
  // 2026-10-11-clax-dev-hosted-design §16.3): "Mia is typing…" while their
  // keystrokes are fresh, and their draft as it stands, in their live colour.
  // Drafts are never stored: this goes when they post, cancel or leave.
  let { writers }: { writers: LiveWriter[] } = $props();
  const swatch = (c: number) => `var(--live-${((c % 8) + 8) % 8})`;
</script>

{#if writers.length}
  <ul class="live-writers" aria-label="Writing now">
    {#each writers as w (w.person_id)}
      <li style:--lc={swatch(w.color)}>
        <p class="lw-who"><span class="lw-dot" aria-hidden="true"></span><b>{w.name}</b><span class="lw-st">{w.typing ? "is typing…" : "has a draft"}</span></p>
        {#if w.text}<p class="lw-draft">{w.text}</p>{/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  /* Live colours, one per person on the artifact, here rather than in
     theme.css so they load with the first writer. Light: 5.4:1 or more on
     --card (the content frame and the name tags draw this set in both
     themes: LIVE_PALETTE in view/live-layers.ts). Dark: 7.1:1 or more on --card. */
  :global {
    :root {
      --live-0: #2f62d0; --live-1: #8a3fd1; --live-2: #0a757b; --live-3: #be2f7a;
      --live-4: #975a00; --live-5: #6b6500; --live-6: #33708f; --live-7: #a3445a;
    }
    :root:not([data-theme="light"]) { @media (prefers-color-scheme: dark) {
      --live-0: #7fa6ff; --live-1: #c39bff; --live-2: #4fd0d6; --live-3: #ff8cc6;
      --live-4: #f0b155; --live-5: #d6cd4f; --live-6: #7fc2e3; --live-7: #ff9aae;
    } }
    :root[data-theme="dark"] {
      --live-0: #7fa6ff; --live-1: #c39bff; --live-2: #4fd0d6; --live-3: #ff8cc6;
      --live-4: #f0b155; --live-5: #d6cd4f; --live-6: #7fc2e3; --live-7: #ff9aae;
    }
    .live-writers { list-style: none; margin: 10px 0 0; padding: 0; display: grid; gap: 8px; }
    .new-writers .live-writers { margin: 0 0 10px; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius); background: var(--card); }
    .live-writers li { border-left: 2px dashed var(--lc); padding: 1px 0 1px 10px; min-width: 0; }
    .live-writers .lw-who { display: flex; align-items: center; gap: 6px; margin: 0; font: 400 12px/18px var(--font); color: var(--muted); min-width: 0; }
    .live-writers .lw-who b { font-weight: 600; color: var(--fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
    .live-writers .lw-st { white-space: nowrap; }
    .live-writers .lw-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--lc); flex: none; }
    .live-writers .lw-draft { margin: 2px 0 0; font-size: 13.5px; line-height: 1.55; color: var(--muted); white-space: pre-wrap; overflow-wrap: anywhere; max-height: 9.3em; overflow: hidden; }
    @media (prefers-reduced-motion: no-preference) { .live-writers .lw-dot { animation: lw-pulse 1.6s ease-in-out infinite; } }
    @keyframes lw-pulse { 50% { opacity: .35; } }
  }
</style>
