<script lang="ts">
  // What the stage shows in place of the frame when no page can open
  // there: the browser would not let the page sign in in a frame (it keeps
  // the frame's cookies apart and refused them), so the page's own address
  // is offered, to open in a tab, where it signs in on its own; or (`href`
  // null) the shell is reached from another machine, and a local daemon's
  // pages open on its machine alone.
  let { href }: { href: string | null } = $props();
</script>

{#if href}
  <p class="empty" role="status">This browser would not open the page here. <a {href} target="_blank" rel="noopener">Open in a tab</a></p>
{:else}
  <p class="empty" role="status">This page opens only on the machine that runs Clax.</p>
{/if}
