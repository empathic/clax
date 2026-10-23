<svelte:options css="injected" />

<script lang="ts">
  // The frame of ClaxBot's pages in the gallery shell (`/admin`, `/settings`,
  // `/me`, `/audit`): a heading (focused when the page opens, for a screen
  // reader), a line under it, an optional row of tabs (links, the current
  // one marked) and the page. Its styles are the pages' shared ones: cards,
  // tables that scroll inside their card at phone width rather than the
  // page, fields, and the error and status lines.
  import { onMount, type Snippet } from "svelte";

  let { title, lede, tabs, current, actions, children }: {
    title: string; lede?: string; tabs?: { id: string; label: string; href: string }[]; current?: string; actions?: Snippet; children: Snippet;
  } = $props();
  let head: HTMLElement | undefined = $state();
  onMount(() => head?.focus({ preventScroll: true }));
</script>

<div class="bpg">
  <div class="bpg-head">
    <h2 tabindex="-1" bind:this={head}>{title}</h2>
    <span class="sp"></span>
    {@render actions?.()}
  </div>
  {#if lede}<p class="bpg-lede">{lede}</p>{/if}
  {#if tabs}
    <nav class="bpg-tabs" aria-label={`${title} sections`}>
      {#each tabs as t (t.id)}<a href={t.href} aria-current={t.id === current ? "page" : undefined}>{t.label}</a>{/each}
    </nav>
  {/if}
  {@render children()}
</div>

<style>
  :global {
    .bpg { max-width: 1040px; margin: 0 auto; min-width: 0; display: flex; flex-direction: column; gap: 14px; }
    .bpg-head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
    .bpg h2 { margin: 0; font-size: 22px; line-height: 1.2; }
    .bpg h2:focus { outline: none; }
    .bpg .sp { flex: 1; }
    .bpg-lede { margin: 0; color: var(--muted); font-size: 13.5px; line-height: 1.55; max-width: 72ch; }
    .bpg-tabs { display: flex; flex-wrap: wrap; gap: 4px; }
    .bpg-tabs a { padding: 6px 10px; border-radius: var(--radius-sm); font-weight: 600; font-size: 13.5px; color: var(--muted); }
    .bpg-tabs a:hover { background: var(--hover); color: var(--fg); }
    .bpg-tabs a[aria-current="page"] { background: var(--hover); color: var(--fg); box-shadow: inset 0 -2px 0 var(--accent); }
    .bpg .bcard { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px 16px; min-width: 0; }
    .bpg h3 { margin: 0 0 8px; font-size: 16px; }
    .bpg .bcard > p { margin: 0 0 10px; font-size: 13px; color: var(--muted); line-height: 1.5; }
    .bpg .err { margin: 0; color: var(--danger); font-size: 13px; }
    .bpg .note { margin: 0; color: var(--muted); font-size: 13px; }
    .bpg .ok { margin: 0; color: var(--accent-ink); font-size: 13px; }
    .bpg .stats { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 10px; }
    .bpg .stat { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 10px 12px; min-width: 0; }
    .bpg .stat b { display: block; font-size: 20px; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; }
    .bpg .stat span { display: block; font-size: 12px; color: var(--muted); }
    .bpg .scroll { overflow-x: auto; min-width: 0; }
    .bpg table { width: 100%; border-collapse: collapse; font-size: 13.5px; }
    .bpg th { text-align: start; font-size: 12px; font-weight: 600; color: var(--muted); padding: 6px 8px; border-bottom: 1px solid var(--border); white-space: nowrap; }
    .bpg td { padding: 8px; border-bottom: 1px solid var(--border); vertical-align: middle; overflow-wrap: anywhere; }
    .bpg tr:last-child td { border-bottom: 0; }
    .bpg td.num, .bpg th.num { text-align: end; font-variant-numeric: tabular-nums; white-space: nowrap; }
    .bpg .fields { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 12px 16px; }
    .bpg .field { display: grid; gap: 4px; min-width: 0; align-content: start; }
    .bpg .field > label, .bpg .field > .lbl { font-size: 12.5px; font-weight: 600; color: var(--muted); display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
    .bpg .field input, .bpg .field select { width: 100%; min-width: 0; }
    .bpg .field small { font-size: 12px; color: var(--muted); }
    .bpg .acts { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
    .bpg input[type="number"] { font-variant-numeric: tabular-nums; }
    .bpg td input, .bpg td select { max-width: 100%; }
    .bpg button { max-width: 100%; }
    @media (max-width: 700px) {
      .bpg .bcard { padding: 12px; }
      .bpg h2 { font-size: 20px; }
    }
  }
</style>
