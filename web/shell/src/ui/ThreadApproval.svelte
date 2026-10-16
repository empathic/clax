<svelte:options css="injected" />

<script lang="ts">
  // This viewer's approval card for a thread waiting for their approval
  // (spec 2026-10-08-multi-agent-multi-person §9.5), above the thread in
  // the threads panel: the inbox's own card. Fetched again whenever the
  // thread's delivery state or comments change. Once decided, the card
  // goes; focus left in it goes to its thread's card head. A thread that
  // reads "waiting for your approval" with no request for this viewer (its
  // agent was told of the withheld comment but has not asked) says that
  // nothing waits for a decision yet, and where one will come.
  import { tick } from "svelte";
  import type { InboxItem } from "../api";
  import ApprovalCard from "../people/ApprovalCard.svelte";
  import type { Thread } from "../threads";
  import { approvalFor } from "./thread-approvals";

  let { t, now }: { t: Thread; now: Date } = $props();
  let item = $state<InboxItem | null>(null);
  let asked = $state(false);
  $effect(() => {
    const id = t.id;
    void [t.feedback_state?.state, t.feedback_state?.since, t.comments.length];
    let live = true;
    void approvalFor(t.artifact_id, id).then(i => {
      if (!live) return;
      const held = !i && document.activeElement?.closest<HTMLElement>(".thread-approval")?.dataset.thread === id;
      item = i;
      asked = true;
      if (held) void tick().then(() => [...document.querySelectorAll<HTMLElement>(".thread-card")].find(c => c.dataset.thread === id)?.querySelector<HTMLElement>(".card-head")?.focus());
    }, () => {});
    return () => { live = false; };
  });
</script>

{#if item}<div class="thread-approval" data-thread={t.id}><ApprovalCard {item} {now} /></div>
{:else if asked && t.feedback_state?.state === "withheld" && t.feedback_state.mine}<p class="thread-approval none">Nothing to decide yet: your agent has not asked to see this. When it asks, the request comes to your <a href="/inbox?kind=approval">inbox</a> and shows here.</p>{/if}

<style>
  :global { .thread-approval { margin-bottom: 6px; } .thread-approval.none { margin: 0 0 6px; padding: 6px 10px; border: 1px dashed var(--border-strong); border-radius: var(--radius); color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; } .thread-approval.none a { color: var(--fg); } }
</style>
