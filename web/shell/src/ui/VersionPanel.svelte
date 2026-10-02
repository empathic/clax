<svelte:options css="injected" />

<script lang="ts">
  // The version menu's panel (spec §8): the versions newest first, as a
  // changelog. Escape or a press outside closes it.
  import { type RowInput, excerpt, versionRows } from "../view/version-rows";

  let { input, hrefFor, onChoose, onClose }: { input: RowInput; hrefFor(n: number): string; onChoose(n: number): void; onClose(): void } = $props();
  const rows = $derived(versionRows(input));
  const byId = $derived(new Map(input.threads.map(t => [t.id, t])));
  let panel: HTMLDivElement | undefined = $state();
  $effect(() => { panel?.querySelector<HTMLElement>("a[aria-current=page]")?.focus(); });
  function choose(e: MouseEvent, n: number) {
    // A plain click moves through the controller, as the select did; a
    // modified click keeps the link's own behaviour (a new tab).
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    onChoose(n);
  }
  function outside(e: PointerEvent) { if (panel && !panel.contains(e.target as Node) && !(e.target as Element).closest?.(".vbtn")) onClose(); }
</script>

<svelte:window onpointerdown={outside} />
<!-- Escape closes the dialog. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="vmenu" role="dialog" aria-label="Versions" tabindex="-1" bind:this={panel}
  onkeydown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); } }}>
  <ol>
    {#each rows as r (r.n)}
      <li class={["vrow", r.current && "cur"]}>
        <a href={hrefFor(r.n)} aria-current={r.current ? "page" : undefined} onclick={e => choose(e, r.n)}>
          <span class="g">v{r.n}</span>
          <span class="h">{r.who}<small>{r.when}{r.label ? ` · ${r.label}` : ""}</small></span>
          {#if r.chips.length}<span class="cl">Addressed {#each r.chips as c (c.id)}<span class={["pc", c.open && "open"]} title={byId.get(c.id) ? excerpt(byId.get(c.id)!) : undefined}><i>{c.n ?? "•"}</i></span>{/each}</span>{/if}
          {#if r.did}<span class="cl">{r.did}</span>{/if}
          {#if r.note}<span class="cl note">{r.note}</span>{/if}
        </a>
      </li>
    {/each}
  </ol>
</div>

<!-- The panel's styles travel with its lazy chunk. -->
<style>
  :global {
    .vmenu { position: absolute; right: 0; top: calc(100% + 8px); z-index: 20; width: 440px; max-height: 70vh; overflow: auto; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); padding: 6px 0; }
    .vmenu ol { list-style: none; margin: 0; padding: 0; }
    .vrow a { display: grid; grid-template-columns: 48px 1fr; gap: 2px 10px; padding: 9px 16px; border-bottom: 1px solid var(--border); }
    .vrow:last-child a { border-bottom: 0; }
    .vrow a:hover, .vrow a:focus-visible { background: var(--bg); outline: none; }
    .vrow .g { font-size: 24px; line-height: 1; grid-row: span 4; }
    .vrow.cur a { background: var(--bg); } .vrow.cur .g { color: var(--agent-ink); }
    .vrow .h { font: 600 14px var(--grot); } .vrow .h small { font: 400 11.5px var(--mono); color: var(--muted); margin-left: 6px; }
    .vrow .cl { display: flex; flex-wrap: wrap; gap: 4px 10px; font-size: 12px; color: var(--muted); align-items: center; }
    .pc i { font-style: normal; display: inline-block; width: 17px; height: 17px; border-radius: 50%; font: 600 10px/17px var(--mono); text-align: center; background: var(--card); color: var(--fg); box-shadow: inset 0 0 0 1.5px var(--agent); }
    .pc.open i { box-shadow: inset 0 0 0 1.5px var(--you); }
    @media (max-width: 700px) { .vmenu { position: fixed; left: 0; right: 0; top: 56px; bottom: 52px; width: auto; max-height: none; box-shadow: none; border-width: 1px 0 0; } }
  }
</style>
