<script lang="ts" generics="V">
  // Retry on a card's line of what its artifact's link did not send (a
  // thread card's, a question card's): `retryCopies`, loaded with the first
  // press. `view` is the card's view of its thread or question: `onRetried`
  // hears the view Retry succeeded on (until a newer view comes, the card
  // reads what was not sent as sending again: the retry moves no event).
  // The view a Retry failed on, and why, is said beside the button until
  // the next press or a newer view. The card styles `.retry` and `.retry-err`.
  let { view, artifact, remote, stop = false, onRetried }: {
    view: V; artifact: string; remote: string;
    /** The press is the button's alone, not the card's under it. */
    stop?: boolean;
    onRetried(view: V): void;
  } = $props();
  let retrying = $state(false);
  let failed = $state.raw<{ on: V; text: string } | null>(null);

  async function retry() {
    if (retrying) return;
    retrying = true;
    failed = null;
    const on = view;
    try {
      const text = await (await import("./retry-request")).retryCopies(artifact, remote);
      if (text) failed = { on, text }; else onRetried(on);
    } catch { failed = { on, text: "Retry failed: Clax did not answer" }; } finally { retrying = false; }
  }
</script>

{#if failed?.on === view}<span class="retry-err" role="status">{failed.text}</span>{/if}<button type="button" class="retry" disabled={retrying} onclick={e => { if (stop) e.stopPropagation(); void retry(); }}>{retrying ? "Retrying…" : "Retry"}</button>
