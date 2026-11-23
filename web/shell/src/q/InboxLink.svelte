<script lang="ts">
  // **Inbox** with the unread count of the page's inbox feed (spec
  // 2026-10-06-agent-questions-and-inbox §9.4), linking to `/inbox`, in the
  // gallery's header (`ui/InboxAnchor.svelte` draws it), shown to the owner
  // and to a connected person, whose inbox it is.
  import InboxAnchor from "../ui/InboxAnchor.svelte";
  import type { InboxFeed } from "./feed.svelte";
  import { shared } from "./shared";

  const p: { feed?: InboxFeed; current?: boolean } = $props();
  const f = $derived(p.feed ?? shared().inbox);
</script>

{#if f.owner}<InboxAnchor unread={f.unread} current={p.current} />{/if}
