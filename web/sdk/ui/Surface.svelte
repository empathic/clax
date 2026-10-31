<script lang="ts">
  // A raised surface: the overlay's composer, hint and "Your reports" panel.
  // `accent` gives it the people's rule along its top (something a person is
  // writing or doing). Styled only through Clax's tokens
  // (web/sdk/tokens/clax.tokens.json), so a host's theme restyles it. Its
  // attributes are named, not spread: spread attributes and
  // <svelte:element> bring Svelte runtime the overlay's budget has no room
  // for. The caller's class comes first: it names what the surface is.
  import type { Snippet } from "svelte";

  type Props = {
    as?: "div" | "section" | "form";
    accent?: boolean;
    class?: string;
    role?: string;
    label?: string;
    onsubmit?: (e: SubmitEvent) => void;
    children: Snippet;
  };
  let { as = "div", accent = false, class: cls = "", role, label, onsubmit, children }: Props = $props();
  const classes = $derived(`${cls} surface${accent ? " accent" : ""}`.trim());
</script>

{#if as === "form"}
  <form class={classes} aria-label={label} {onsubmit}>{@render children()}</form>
{:else if as === "section"}
  <section class={classes} aria-label={label}>{@render children()}</section>
{:else}
  <div class={classes} {role} aria-label={label}>{@render children()}</div>
{/if}

<style>
  .surface { background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
  .accent { border-top: 3px solid var(--you); }
</style>
