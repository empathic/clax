<script lang="ts">
  // The capture modes (spec 2026-10-29-feedback-loop §4.1): Element (click,
  // Option widens, selecting text gives a range), Area (drag) and Screen
  // (the visible viewport, at once), with E, A and S, and Escape leaving.
  // The keys are the overlay's (Overlay.svelte), heard wherever focus is
  // but in a text field; E again in Element mode (or its button pressed
  // again) starts picking from the keyboard. The bar is a toolbar: Left and
  // Right move focus between its buttons (wrapping), Home and End to the
  // first and last. `mode` is null when none is on yet (a "Still broken"
  // note, before its new capture).
  import type { Mode } from "./capture";

  type Props = { mode: Mode | null; outlined?: string | null; onMode(m: Mode): void; onCancel(): void };
  let { mode, outlined = null, onMode, onCancel }: Props = $props();

  /** Focus goes to the mode that is on (none for a reopen's note, whose
   * text field keeps it). */
  const focusOn = (el: HTMLButtonElement, m: Mode) => { if (mode === m) el.focus(); };

  const MODES: { mode: Mode; label: string; key: string }[] = [
    { mode: "element", label: "Element", key: "E" },
    { mode: "area", label: "Area", key: "A" },
    { mode: "screen", label: "Screen", key: "S" },
  ];
  const HINT: Record<Mode, string> = {
    element: "Click what your report is about, or select text; or press E again, then the arrow keys and Enter. Esc cancels.",
    area: "Drag over the part of the page your report is about. Esc cancels.",
    screen: "Taking the visible page…",
  };

  /** The toolbar's arrow keys: Left and Right to the previous and next
   * button (wrapping), Home and End to the first and last. */
  function rove(e: KeyboardEvent): void {
    if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    const bar = e.currentTarget as HTMLElement;
    const buttons = [...bar.querySelectorAll<HTMLButtonElement>("button")];
    const i = buttons.indexOf(e.target as HTMLButtonElement);
    if (i < 0) return;
    const n = buttons.length;
    const to = e.key === "ArrowRight" ? (i + 1) % n : e.key === "ArrowLeft" ? (i + n - 1) % n : e.key === "Home" ? 0 : e.key === "End" ? n - 1 : -1;
    if (to < 0) return;
    e.preventDefault();
    buttons[to].focus();
  }
</script>

<div class="modebar" role="toolbar" aria-label="Capture" tabindex="-1" onkeydown={rove}>
  <div class="modes">
    {#each MODES as m (m.mode)}
      <button type="button" {@attach el => focusOn(el, m.mode)} aria-pressed={mode === m.mode} aria-keyshortcuts={m.key} onclick={() => onMode(m.mode)}>{m.label}<kbd aria-hidden="true">{m.key}</kbd></button>
    {/each}
  </div>
  <span class="hint" role="status">{mode ? HINT[mode] : "Add a screenshot: pick a mode, or just write your note."}</span>
  <button type="button" class="ghost" aria-keyshortcuts="Escape" onclick={onCancel}>Cancel</button>
  <span class="outlined" aria-live="polite">{mode === "element" && outlined ? `Outlined ${outlined}.` : ""}</span>
</div>

<style>
  .modebar { position: fixed; left: 50%; top: 12px; transform: translateX(-50%); width: max-content; max-width: calc(100vw - 32px); display: flex; flex-wrap: wrap; gap: 6px 10px; align-items: center; padding: 6px; background: var(--raised); border: 1px solid var(--border-hover); border-top: 3px solid var(--you); border-radius: var(--radius); box-shadow: var(--elev-lg); font-size: 13px; z-index: 3; pointer-events: auto; }
  .modes { display: flex; gap: 4px; }
  .modes button { min-height: 30px; padding: 5px 10px; }
  .modes button[aria-pressed="true"] { background: var(--you); color: var(--on-you); border-color: var(--you); }
  /* The key, shown beside the label and kept out of its accessible name (aria-keyshortcuts tells it). */
  kbd { font: 500 11px/1 var(--font); padding: 2px 4px; border: 1px solid currentColor; border-radius: var(--radius-xs); opacity: .7; }
  .outlined { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .hint { flex: 1 1 180px; color: var(--muted); }
</style>
