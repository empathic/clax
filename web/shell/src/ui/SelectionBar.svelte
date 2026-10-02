<svelte:options css="injected" />

<script lang="ts">
  // The selection bar (spec §8): the ticked threads, sent together to one
  // agent, with an optional note. It sits at the top of the sidebar and stays
  // in view while the list scrolls. `send` renders the shared Send button and
  // is given the guarded send: a Send from the keyboard on a trail the page
  // may have steered does nothing and says how to act instead (`guardedAction`).
  import type { Snippet } from "svelte";
  import { countLabel } from "../view/batch-model";
  import { isSubmitKey } from "../view/composer-model";
  import { guardedAction } from "../view/trail";

  let { count, note, busy, send, onNote, onClear, onSend }: {
    count: number; note: string; busy: boolean; send: Snippet<[(e: Event) => void]>; onNote(v: string): void; onClear(): void; onSend(): void;
  } = $props();
  let hint: string | null = $state(null);
  const go = (e: Event) => { hint = guardedAction(e, "send", onSend); };
</script>

<div class="selbar" role="region" aria-label="Selected comments">
  <div class="selbar-row">
    <span class="converge" aria-hidden="true"><i></i><i></i><i></i><i></i></span>
    <span class="txt"><b role="status">{countLabel(count)}</b>sent together</span>
    <button type="button" class="ghost" onclick={onClear}>Clear</button>
    {@render send(go)}
  </div>
  <input class="selbar-note" aria-label="Note for the agent (optional)" placeholder="Note for the agent (optional)" maxlength="280" value={note} disabled={busy}
    oninput={e => onNote(e.currentTarget.value)} onkeydown={e => { if (isSubmitKey(e)) { e.preventDefault(); go(e); } }} />
  <!-- Said when the keyboard asked for Send on a trail the page may have steered; the count is the bar's status. -->
  <p class="act-hint" aria-live="polite">{hint ?? ""}</p>
</div>

<!-- Its styles travel with its own lazy chunk. -->
<style>
  :global {
    .selbar { position: sticky; top: 0; z-index: 5; margin: 0 -14px 8px; display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; background: var(--raised); border-bottom: 1px solid var(--border-strong); box-shadow: 0 8px 24px var(--shadow); }
    .selbar-row { display: flex; align-items: center; gap: 12px; }
    .selbar .txt { flex: 1; font-size: 12px; color: var(--muted); line-height: 1.35; }
    .selbar .txt b { display: block; color: var(--fg); font: 600 16px/1.1 var(--grot); }
    .selbar .act-hint { margin: 0; }
    .converge { position: relative; width: 46px; height: 24px; flex: none; }
    .converge i { position: absolute; top: 5px; width: 14px; height: 14px; border-radius: 50%; background: var(--you); border: 1.5px solid var(--raised); transition: left .4s cubic-bezier(.4,0,.2,1); }
    .converge i:nth-child(1) { left: 0; } .converge i:nth-child(2) { left: 8px; } .converge i:nth-child(3) { left: 16px; }
    .converge i:nth-child(4) { left: 28px; top: 2px; width: 20px; height: 20px; background: var(--agent); }
    @media (max-width: 700px) { .selbar-row { flex-wrap: wrap; } .selbar button { min-height: 40px; } }
    @media (prefers-reduced-motion: reduce) { .converge i { transition: none; } }
  }
</style>
