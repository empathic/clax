<svelte:options css="injected" />

<script lang="ts">
  // This viewer's approval card for a thread waiting for their approval
  // (spec 2026-10-08-multi-agent-multi-person §9.5), above the thread in
  // the threads panel: the inbox's own card. Fetched again whenever the
  // thread's delivery state or comments change; `onShown` hears whether a card shows.
  import { untrack } from "svelte";
  import type { InboxItem } from "../api";
  import ApprovalCard from "../people/ApprovalCard.svelte";
  import type { Thread } from "../threads";
  import { approvalFor } from "./thread-approvals";

  let { t, now, onShown }: { t: Thread; now: Date; onShown(id: string, shown: boolean): void } = $props();
  let item = $state<InboxItem | null>(null);
  $effect(() => {
    const id = t.id;
    void [t.feedback_state?.state, t.feedback_state?.since, t.comments.length];
    let live = true;
    void approvalFor(t.artifact_id, id).then(i => { if (!live) return; item = i; onShown(id, !!i); }, () => {});
    return () => { live = false; };
  });
  $effect(() => { const id = untrack(() => t.id); return () => onShown(id, false); });
</script>

{#if item}<div class="thread-approval"><ApprovalCard {item} {now} /></div>{/if}

<style>
  :global { .thread-approval { margin-bottom: 6px; } }
</style>
