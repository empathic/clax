<script lang="ts">
  // A button: quiet by default, `primary` in solid ink (the one action that
  // moves things on), `ghost` with no fill until hovered, `pill` rounded
  // fully (a launcher's). `pressed` and `expanded` set the ARIA states of a
  // toggle and of a button that opens something. Styled only through Clax's
  // tokens.
  import type { Snippet } from "svelte";

  type Props = {
    variant?: "default" | "primary" | "ghost";
    pill?: boolean;
    type?: "button" | "submit";
    disabled?: boolean;
    pressed?: boolean;
    expanded?: boolean;
    label?: string;
    class?: string;
    onclick?: (e: MouseEvent) => void;
    /** The <button>, for a caller that gives focus back to it. */
    ref?: HTMLButtonElement | null;
    children: Snippet;
  };
  let { variant = "default", pill = false, type = "button", disabled = false, pressed, expanded, label, class: cls = "", onclick, ref = $bindable(null), children }: Props = $props();
  const classes = $derived(`${cls} btn${variant === "default" ? "" : ` ${variant}`}${pill ? " pill" : ""}`.trim());
</script>

<button {type} class={classes} {disabled} aria-pressed={pressed} aria-expanded={expanded} aria-label={label} {onclick} bind:this={ref}>{@render children()}</button>

<style>
  .btn { font: 500 14px/1.15 var(--font); min-height: 32px; padding: 7px 12px; margin: 0; background: var(--card); color: var(--fg); border: 1px solid var(--border); border-radius: var(--radius-sm); cursor: pointer; display: inline-flex; align-items: center; justify-content: center; gap: 8px; white-space: nowrap; transition: color var(--t), background-color var(--t), border-color var(--t); }
  .btn:not(:disabled):hover { background-color: var(--hover); border-color: var(--border-hover); }
  .btn:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
  .primary { background: var(--fg); color: var(--bg); border-color: var(--fg); }
  .primary:not(:disabled):hover { background: var(--primary-hover); border-color: var(--primary-hover); }
  .ghost { background: none; border-color: transparent; }
  .ghost:not(:disabled):hover { border-color: transparent; }
  .pill { border-radius: 999px; padding: 7px 14px; }
  .btn:disabled { opacity: .5; cursor: default; }
  @media (pointer: coarse) { .btn { min-height: 40px; } }
</style>
