<svelte:options css="injected" />

<script lang="ts">
  // Settings' search: a field and the list of what it finds among the
  // screens this browser sees (`screens.ts` `search`), each result naming
  // where it is ("People › People"). Arrow keys, Home and End move through
  // the list while the field keeps the focus; Enter or a click opens the
  // one marked. With `all`, the screens are listed before anything is
  // typed, so the list also serves as the way around Settings; without it
  // the list shows once there is a query. A result is a link, so it opens
  // in a new tab as any link does.
  import { type Ctx, type Hit, search } from "./screens";

  let { ctx, all = false, focus = false, query = $bindable(""), onGo }: {
    ctx: Ctx; all?: boolean; focus?: boolean; query?: string; onGo(href: string): void;
  } = $props();
  const hits = $derived<Hit[]>(all || query.trim() ? search(query, ctx) : []);
  let at = $state(0);
  let list: HTMLElement | undefined = $state();
  const uid = $props.id();
  const open = $derived(all || !!query.trim());
  const marked = $derived(hits[Math.min(at, hits.length - 1)]);
  const optId = (i: number) => `${uid}-o${i}`;
  const move = (to: number) => {
    at = Math.max(0, Math.min(hits.length - 1, to));
    document.getElementById(optId(at))?.scrollIntoView?.({ block: "nearest" });
  };
  function keydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown") { e.preventDefault(); move(at + 1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); move(at - 1); }
    else if (e.key === "Home" && hits.length) { e.preventDefault(); move(0); }
    else if (e.key === "End" && hits.length) { e.preventDefault(); move(hits.length - 1); }
    else if (e.key === "Enter" && marked) { e.preventDefault(); onGo(marked.href); }
  }
  const pick = (e: MouseEvent, h: Hit) => {
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    onGo(h.href);
  };
  const focused = (el: HTMLElement) => { if (focus) el.focus(); };
</script>

<div class="ssearch">
  <input type="search" role="combobox" class="ssearch-in" placeholder="Search settings" aria-label="Search settings" autocomplete="off" spellcheck="false"
    aria-expanded={open} aria-controls={`${uid}-l`} aria-autocomplete="list" aria-activedescendant={open && marked ? optId(Math.min(at, hits.length - 1)) : undefined}
    value={query} oninput={e => { query = e.currentTarget.value; at = 0; }} onkeydown={keydown} {@attach focused} />
  {#if open}
    <ul class="ssearch-list" role="listbox" id={`${uid}-l`} aria-label="Settings found" bind:this={list}>
      {#each hits as h, i (h.key)}
        <li role="option" id={optId(i)} aria-selected={h === marked} onpointermove={() => { at = i; }}>
          <a href={h.href} tabindex="-1" onclick={e => pick(e, h)}><span class="t">{h.title}</span><span class="w">{h.where}</span></a>
        </li>
      {/each}
    </ul>
    <p class="ssearch-none" role="status">{hits.length ? "" : "Nothing in Settings matches."}</p>
  {/if}
</div>

<style>
  :global {
    .ssearch { display: flex; flex-direction: column; gap: 8px; min-width: 0; min-height: 0; }
    .ssearch-in { width: 100%; min-width: 0; }
    .ssearch-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; overflow-y: auto; min-height: 0; }
    .ssearch-list a { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 10px; padding: 8px 10px; border-radius: var(--radius-sm); color: var(--fg); text-decoration: none; min-height: 36px; }
    .ssearch-list [aria-selected="true"] a { background: var(--hover); box-shadow: inset 2px 0 0 var(--accent); }
    .ssearch-list .t { font-weight: 600; font-size: 14px; overflow-wrap: anywhere; }
    .ssearch-list .w { color: var(--muted); font-size: 12.5px; margin-left: auto; }
    .ssearch-none { margin: 0; color: var(--muted); font-size: 13px; }
    .ssearch-none:empty { display: none; }
    @media (pointer: coarse) { .ssearch-list a { min-height: 44px; } }
  }
</style>
