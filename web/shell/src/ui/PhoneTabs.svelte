<script lang="ts">
  // The phone's Page | Threads switch at the foot of the view: two toggle
  // buttons, rendered after the sidebar so the Tab order follows the screen;
  // then, for a browser with an inbox, Inbox (the top bar's, at phone width),
  // known as the view is (`view/deciders.svelte.ts`), so the tabs are drawn once.
  import { decided } from "../view/deciders.svelte";
  import InboxAnchor from "./InboxAnchor.svelte";

  let { panel, open, onPage, onThreads }: { panel: boolean; open: number; onPage(): void; onThreads(): void } = $props();
</script>

<div class="phone-tabs" role="group" aria-label="Page or threads">
  <button type="button" aria-pressed={!panel} onclick={onPage}>Page</button>
  <button type="button" data-shell-control="threads" aria-pressed={panel} onclick={onThreads}>Threads <span class="cnt">{open}</span></button>
  {#if decided.inbox}<InboxAnchor unread={decided.unread} />{/if}
</div>
