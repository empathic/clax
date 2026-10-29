<script lang="ts">
  // The capture modes (spec 2026-10-29-feedback-loop §4.1): Element (click,
  // Option widens, selecting text gives a range), Area (drag) and Screen
  // (the visible viewport, at once), with E, A and S, and Escape leaving.
  // The keys are the overlay's (Overlay.svelte), heard wherever focus is
  // but in a text field. `mode` is null when none is on yet (a "Still
  // broken" note, before its new capture).
  import type { Mode } from "./capture";

  type Props = { mode: Mode | null; onMode(m: Mode): void; onCancel(): void };
  let { mode, onMode, onCancel }: Props = $props();

  const MODES: { mode: Mode; label: string; key: string }[] = [
    { mode: "element", label: "Element", key: "E" },
    { mode: "area", label: "Area", key: "A" },
    { mode: "screen", label: "Screen", key: "S" },
  ];
  const HINT: Record<Mode, string> = {
    element: "Click what your report is about, or select text. Esc cancels.",
    area: "Drag over the part of the page your report is about. Esc cancels.",
    screen: "Taking the visible page…",
  };
</script>

<div class="modebar" role="toolbar" aria-label="Capture">
  <div class="modes">
    {#each MODES as m (m.mode)}
      <button type="button" aria-pressed={mode === m.mode} aria-keyshortcuts={m.key} data-key={m.key} onclick={() => onMode(m.mode)}>{m.label}</button>
    {/each}
  </div>
  <span class="hint" role="status">{mode ? HINT[mode] : "Add a screenshot: pick a mode, or just write your note."}</span>
  <button type="button" class="ghost" aria-keyshortcuts="Escape" onclick={onCancel}>Cancel</button>
</div>

<style>
  .modebar { position: fixed; left: 50%; top: 12px; transform: translateX(-50%); width: max-content; max-width: calc(100vw - 32px); display: flex; flex-wrap: wrap; gap: 6px 10px; align-items: center; padding: 6px; background: var(--raised); border: 1px solid var(--border-hover); border-top: 3px solid var(--you); border-radius: var(--radius); box-shadow: var(--elev-lg); font-size: 13px; z-index: 3; pointer-events: auto; }
  .modes { display: flex; gap: 4px; }
  .modes button { min-height: 30px; padding: 5px 10px; }
  .modes button[aria-pressed="true"] { background: var(--you); color: var(--on-you); border-color: var(--you); }
  /* The key, drawn beside the label and kept out of its accessible name (aria-keyshortcuts tells it). */
  .modes button::after { content: attr(data-key); font: 500 11px/1 var(--font); padding: 2px 4px; border: 1px solid currentColor; border-radius: var(--radius-xs); opacity: .7; }
  .hint { flex: 1 1 180px; color: var(--muted); }
</style>
