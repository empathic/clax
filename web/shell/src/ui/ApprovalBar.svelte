<svelte:options css="injected" />
<script lang="ts">
  // The approval bar: what the shown version declares that waits for the
  // viewer's approval (the daemon's `pending_capabilities`, shown only to a
  // viewer it says may approve). Review opens the dialog that says what each
  // item lets the page do (ApprovalDialog), where the viewer approves or
  // hides the bar for this view.
  import type { PendingCapabilities } from "../api";
  import { barText } from "../caps/approval";

  let { pending, onReview }: { pending: PendingCapabilities; onReview(): void } = $props();
</script>

<div class="banner approval" role="status" data-approval-bar>
  <span>{barText(pending)}</span>
  <button type="button" onclick={onReview}>Review</button>
</div>

<style>
  /* In the stage's foot (StageIsland, theme.css .stage-foot). */
  .approval { font-size: 13px; }
  .approval span { min-width: 0; }
  @media (max-width: 480px) { .approval { align-self: stretch; flex-wrap: wrap; } }
</style>
