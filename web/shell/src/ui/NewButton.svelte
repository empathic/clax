<svelte:options css="injected" />

<script lang="ts">
  // New in the gallery's header (docs/contract.md "Starting an artifact
  // from a brief"), shown once `GET /api/viewers/me` says this browser may
  // start an artifact (`may_create`: the owner's, or a connected person's
  // whose rights create). It loads with the gallery's lazy module, so the
  // gallery's first paint carries none of it, and its dialog loads on the
  // first press.
  import type { Me } from "../view/new-artifact";

  let me = $state<Me>(null);
  let may = $state(false);
  let Dialog = $state<typeof import("./NewArtifactDialog.svelte").default | null>(null);
  let open = $state(false);
  let failed = $state(false);
  void fetch("/api/viewers/me").then(r => r.json()).then((b: { viewer?: Me; may_create?: boolean }) => { me = b?.viewer ?? null; may = b?.may_create === true; }, () => {});
  const show = () => { void import("./NewArtifactDialog.svelte").then(m => { Dialog = m.default; open = true; failed = false; }, () => { failed = true; }); };
</script>

{#if may}
  <button type="button" class="new" aria-haspopup="dialog" aria-label="New artifact" title={failed ? "The dialog did not load: try again" : "Start an artifact from a brief"} onclick={show}>
    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" /></svg><span class="word">New</span>
  </button>
{/if}
{#if open && Dialog}<Dialog {me} onClose={() => { open = false; }} />{/if}

<style>
  .new { flex: none; gap: 6px; padding: 0 10px 0 8px; font-weight: 600; }
  svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; }
  /* At phone width beside the search box, the icon alone, and Inbox's too,
     so the search box keeps its room. */
  @media (max-width: 520px) { .new { width: 32px; padding: 0; } .word { display: none; } :global(.gbar:has(.new) .inbox-link .word) { display: none; } }
</style>
