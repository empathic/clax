<svelte:options css="injected" />

<script lang="ts">
  // A thread's clip: a thumbnail that opens the whole clip over the page,
  // closed by Escape, a click outside it or its Close button. `src` is the
  // clip's URL; `load`, when given instead, fetches it once the thumbnail
  // nears the view (the side panel's clips come through the worker, which
  // holds the credential they are served with).
  let { src = null, load }: { src?: string | null; load?: () => Promise<string | null> } = $props();
  let url = $state<string | null>(null);
  let dlg: HTMLDialogElement | undefined = $state();
  let thumb: HTMLButtonElement | undefined = $state();
  const shown = $derived(load ? url : src);
  const near = (el: HTMLElement) => {
    if (!load) return;
    const go = () => { void load().then(u => { url = u; }, () => {}); };
    if (typeof IntersectionObserver !== "function") { go(); return; }
    const io = new IntersectionObserver(([e]) => { if (e.isIntersecting) { io.disconnect(); go(); } }, { rootMargin: "200px" });
    io.observe(el);
    return () => io.disconnect();
  };
  function open(): void {
    if (!dlg) return;
    if (typeof dlg.showModal === "function") dlg.showModal(); else dlg.open = true;
  }
  function close(): void {
    if (dlg?.open) { if (typeof dlg.close === "function") dlg.close(); else dlg.open = false; }
    thumb?.focus();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="clip" {@attach near} onclick={e => e.stopPropagation()}>
  {#if shown}
    {@const s = shown}
    <button type="button" class="clip-thumb" bind:this={thumb} aria-label="Enlarge the screenshot" aria-haspopup="dialog" onclick={open}>
      <img class="thumb" src={s} alt="Screenshot of the commented region" loading="lazy" />
    </button>
    <!-- A click on the backdrop (the dialog itself, outside its box) closes it; Escape does too. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <dialog class="clip-view" bind:this={dlg} aria-label="Screenshot of the commented region"
      onclick={e => { if (e.target === dlg) close(); }}
      onkeydown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); } }}>
      <img src={s} alt="Screenshot of the commented region, enlarged" />
      <button type="button" class="ghost clip-close" onclick={close}>Close</button>
    </dialog>
  {/if}
</div>

<style>
  .clip-thumb { display: block; width: auto; max-width: 100%; min-height: 0; padding: 0; margin: 0 0 6px; border: 0; background: none; cursor: zoom-in; }
  .clip-thumb:not(:disabled):hover { background: none; }
  .thumb { display: block; max-width: 100%; max-height: 120px; min-height: 24px; object-fit: cover; object-position: left top; border: 1px solid var(--border); border-radius: var(--radius-xs); background: var(--bg); }
  .clip-view { max-width: min(96vw, 1200px); max-height: 92vh; padding: 8px; border: 1px solid var(--border-hover); border-radius: var(--radius); background: var(--raised); color: var(--fg); box-shadow: var(--elev-lg); }
  .clip-view::backdrop { background: rgb(0 0 0 / 55%); }
  .clip-view img { display: block; max-width: 100%; max-height: calc(92vh - 64px); margin: 0 auto; object-fit: contain; }
  .clip-close { display: block; margin: 8px 0 0 auto; }
</style>
