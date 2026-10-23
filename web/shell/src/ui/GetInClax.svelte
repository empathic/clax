<svelte:options css="injected" />

<script lang="ts">
  // Get in Clax (spec 2026-10-21-app-push-pull §5.5): on an app's page, a
  // link to its get page (`/a/<id>/get`), which opens the Clax app on a
  // phone and offers a QR code, Open in Clax for Mac and Download .clax on a
  // desktop. Its own chunk, loaded only for an artifact that is an app. At
  // phone width the more menu lists it instead (`getHref`). Solid ink, the
  // page's call to action, for visitors; a ghost like the bar's other
  // actions (`ghost`) for whoever may share it, who has the app already.
  let { id, ghost = false }: { id: string; ghost?: boolean } = $props();
</script>

<a class={["get-in-clax", "hide-sm", ghost && "ghost"]} href={`/a/${encodeURIComponent(id)}/get`}>Get in Clax</a>

<style>
  .get-in-clax { display: inline-flex; align-items: center; min-height: 32px; box-sizing: border-box; padding: 7px 12px; border: 1px solid var(--fg); border-radius: var(--radius-sm); background: var(--fg); color: var(--bg); font: 500 14px/1.15 var(--font); white-space: nowrap; text-decoration: none; transition: background-color var(--t), border-color var(--t); }
  .get-in-clax:hover { background: var(--primary-hover); border-color: var(--primary-hover); }
  .get-in-clax.ghost { background: none; border-color: transparent; color: var(--fg); }
  .get-in-clax.ghost:hover { background: var(--hover); border-color: transparent; }
  /* Its own rule beats the bar's .hide-sm, so it hides itself on a phone too. */
  @media (max-width: 700px) { .get-in-clax { display: none; } }
</style>
