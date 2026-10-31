<svelte:options css="injected" />

<script lang="ts">
  // The Retry on a card's not-sent line (ThreadCard, QuestionCard), and
  // before it why the last Retry taken on this view failed (`RetryLink`). A
  // press is the button's own: it does not also select the card. Its styles
  // travel with it (injected on mount), as the question module's do.
  import type { RetryLink } from "./retry-link.svelte";

  type Props = { r: RetryLink<unknown>; view: unknown; artifact: string; remote: string };
  let { r, view, artifact, remote }: Props = $props();
  const error = $derived(r.error(view));
</script>

{#if error}<span class="retry-err" role="status">{error}</span>{/if}<button type="button" class="retry" disabled={r.busy}
  onclick={e => { e.stopPropagation(); void r.run(view, artifact, remote); }}>{r.busy ? "Retrying…" : "Retry"}</button>

<style>
  .retry { margin-left: auto; flex: none; min-height: 24px; padding: 2px 10px; font-size: 12px; font-style: normal; color: var(--fg); }
  .retry-err { font-style: normal; }
</style>
