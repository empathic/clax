<svelte:options css="injected" />

<script lang="ts">
  // One screenshot of a run's content, as a thumbnail that opens full size.
  // Loaded through the shell's fetch (`content-url.ts`), so the owner's
  // browser on a local daemon is not refused for want of its proof.
  import { blobUrl, contentPath } from "./content-url";

  let { run, item, alt }: {
    /** The run whose content holds the image. */
    run: string;
    /** The image's content ID. */
    item: string;
    /** What the image shows, to a screen reader. */
    alt: string;
  } = $props();

  let src = $state<string | null>(null);
  let failed = $state(false);

  $effect(() => {
    const url = contentPath(run, item);
    let gone = false;
    let made: string | null = null;
    src = null; failed = false;
    void blobUrl(url).then(u => {
      if (gone) { URL.revokeObjectURL(u); return; }
      made = u; src = u;
    }, () => { if (!gone) failed = true; });
    return () => { gone = true; if (made) URL.revokeObjectURL(made); };
  });
</script>

{#if src}
  <a href={src} target="_blank" rel="noopener noreferrer"><img {src} {alt} /></a>
{:else if failed}
  <span class="shot-gone">{alt}: not available.</span>
{/if}

<style>
  :global {
    .shot-gone { font-size: 12px; color: var(--muted); }
  }
</style>
