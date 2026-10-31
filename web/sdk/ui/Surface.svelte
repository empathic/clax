<script lang="ts">
  // A raised surface: the overlay's composer, hint and "Your reports" panel.
  // `accent` gives it the people's rule along its top (something a person is
  // writing or doing). Styled only through Clax's tokens
  // (web/sdk/tokens/clax.tokens.json), so a host's theme restyles it.
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  type Props = HTMLAttributes<HTMLElement> & { as?: "div" | "section" | "form"; accent?: boolean; children: Snippet };
  let { as = "div", accent = false, class: cls = "", children, ...rest }: Props = $props();
</script>

<svelte:element this={as} class={["surface", accent && "accent", cls]} {...rest}>{@render children()}</svelte:element>

<style>
  .surface { background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
  .accent { border-top: 3px solid var(--you); }
</style>
