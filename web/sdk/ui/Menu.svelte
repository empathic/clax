<script lang="ts">
  // A menu of actions (WAI-ARIA menu pattern): its first item takes focus
  // when it opens; Up and Down move between items, wrapping, and Home and End
  // go to the ends; Escape closes it; choosing an item calls `onselect` with
  // its ID. A `danger` item is one that destroys something, in the danger
  // colour. Placed by its caller. Styled only through Clax's tokens.
  import type { MenuItem } from "./types";

  type Props = { label: string; items: MenuItem[]; onselect: (id: string) => void; onclose: () => void };
  let { label, items, onselect, onclose }: Props = $props();

  let menu = $state<HTMLElement | null>(null);
  const buttons = () => [...(menu?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];

  $effect(() => { buttons()[0]?.focus(); });

  function onkeydown(e: KeyboardEvent): void {
    const all = buttons();
    const at = all.indexOf(e.target as HTMLButtonElement);
    const go = (i: number) => { e.preventDefault(); all[(i + all.length) % all.length]?.focus(); };
    if (e.key === "ArrowDown") go(at + 1);
    else if (e.key === "ArrowUp") go(at - 1);
    else if (e.key === "Home") go(0);
    else if (e.key === "End") go(all.length - 1);
    else if (e.key === "Escape") { e.preventDefault(); onclose(); }
  }
</script>

<div class="menu" role="menu" aria-label={label} tabindex="-1" bind:this={menu} {onkeydown}>
  {#each items as item (item.id)}
    <button type="button" role="menuitem" class={item.danger ? "item danger" : "item"} disabled={item.disabled} onclick={() => onselect(item.id)}>{item.label}</button>
  {/each}
</div>

<style>
  .menu { display: flex; flex-direction: column; min-width: 160px; padding: 4px; background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
  .item { font: 400 14px/1.3 var(--font); text-align: left; padding: 7px 10px; margin: 0; background: none; color: var(--fg); border: 0; border-radius: var(--radius-xs); cursor: pointer; }
  .item:not(:disabled):hover, .item:focus-visible { background: var(--hover); outline: none; }
  .item:disabled { color: var(--muted); cursor: default; }
  .danger { color: var(--danger); }
</style>
