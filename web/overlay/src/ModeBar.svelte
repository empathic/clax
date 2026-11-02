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
  //
  // It never covers what can be picked (BRIEF clax-feedback-plugins
  // Addendum 4): a compact chip docked to the top or bottom edge, whichever
  // the pointer is farther from, moving to the other edge as the pointer
  // comes near; the full instructions are said in its live region, and only
  // a short hint shows.
  import type { Mode } from "./capture";
  import { edgeFor } from "./keep-clear";

  type Props = { mode: Mode | null; outlined?: string | null; modes?: { mode: Mode; label: string; key: string }[]; onMode(m: Mode): void; onCancel(): void };
  const ALL: { mode: Mode; label: string; key: string }[] = [
    { mode: "element", label: "Element", key: "E" },
    { mode: "area", label: "Area", key: "A" },
    { mode: "screen", label: "Screen", key: "S" },
  ];
  let { mode, outlined = null, modes = ALL, onMode, onCancel }: Props = $props();

  /** Focus goes to the mode that is on (none for a reopen's note, whose
   * text field keeps it). */
  const focusOn = (el: HTMLButtonElement, m: Mode) => { if (mode === m) el.focus(); };

  const HINT: Record<Mode, string> = {
    element: "Click what your report is about, or select text; or press E again, then the arrow keys and Enter. Esc cancels.",
    area: "Drag over the part of the page your report is about. Esc cancels.",
    screen: "Taking the visible page…",
  };
  const SHORT: Record<Mode, string> = { element: "Click or select it", area: "Drag over it", screen: "Taking the page…" };
  const NONE = "Add a screenshot: pick a mode, or just write your note.";

  let bar = $state<HTMLDivElement | null>(null);
  let edge = $state<"top" | "bottom">("top");

  // The chip keeps clear of the pointer, wherever on the page it moves.
  $effect(() => {
    const el = bar;
    if (!el) return;
    const win = el.ownerDocument.defaultView ?? window;
    const onMove = (e: PointerEvent | MouseEvent) => {
      const h = el.getBoundingClientRect().height || 44;
      edge = edgeFor(edge, e.clientY, win.innerHeight, h);
    };
    win.addEventListener("pointermove", onMove, { capture: true, passive: true });
    win.addEventListener("mousemove", onMove, { capture: true, passive: true });
    return () => {
      win.removeEventListener("pointermove", onMove, { capture: true });
      win.removeEventListener("mousemove", onMove, { capture: true });
    };
  });

  /** The toolbar's arrow keys: Left and Right to the previous and next
   * button (wrapping), Home and End to the first and last. */
  function rove(e: KeyboardEvent): void {
    if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    const tb = e.currentTarget as HTMLElement;
    const buttons = [...tb.querySelectorAll<HTMLButtonElement>("button")];
    const i = buttons.indexOf(e.target as HTMLButtonElement);
    if (i < 0) return;
    const n = buttons.length;
    const to = e.key === "ArrowRight" ? (i + 1) % n : e.key === "ArrowLeft" ? (i + n - 1) % n : e.key === "Home" ? 0 : e.key === "End" ? n - 1 : -1;
    if (to < 0) return;
    e.preventDefault();
    buttons[to].focus();
  }
</script>

<div class="modebar" class:bottom={edge === "bottom"} data-edge={edge} role="toolbar" aria-label="Capture" tabindex="-1" onkeydown={rove} bind:this={bar}>
  <div class="modes">
    {#each modes as m (m.mode)}
      <button type="button" {@attach el => focusOn(el, m.mode)} aria-pressed={mode === m.mode} aria-keyshortcuts={m.key} onclick={() => onMode(m.mode)}>{m.label}<kbd aria-hidden="true">{m.key}</kbd></button>
    {/each}
  </div>
  <span class="hint" aria-hidden="true">{mode ? SHORT[mode] : "Pick a mode, or write"} · Esc</span>
  <button type="button" class="ghost" aria-keyshortcuts="Escape" onclick={onCancel}>Cancel</button>
  <span class="sr" role="status">{mode ? HINT[mode] : NONE}</span>
  <span class="sr outlined" aria-live="polite">{mode === "element" && outlined ? `Outlined ${outlined}.` : ""}</span>
</div>

<style>
  .modebar { position: fixed; left: 50%; top: 8px; transform: translateX(-50%); width: max-content; max-width: calc(100vw - 32px); display: flex; flex-wrap: nowrap; gap: 6px; align-items: center; padding: 3px 4px 3px 3px; background: var(--raised); border: 1px solid var(--border-hover); border-radius: 999px; box-shadow: var(--elev-lg); font-size: 12px; z-index: 3; pointer-events: auto; transition: top var(--t), bottom var(--t); }
  .modebar.bottom { top: auto; bottom: 8px; }
  .modes { display: flex; gap: 2px; }
  .modes button, .modebar > button { min-height: 26px; padding: 3px 8px; font-size: 12px; border-radius: 999px; }
  .modes button[aria-pressed="true"] { background: var(--you); color: var(--on-you); border-color: var(--you); }
  /* The key, shown beside the label and kept out of its accessible name (aria-keyshortcuts tells it). */
  kbd { font: 500 10px/1 var(--font); padding: 1px 3px; border: 1px solid currentColor; border-radius: var(--radius-xs); opacity: .7; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .hint { color: var(--muted); white-space: nowrap; }
  @media (prefers-reduced-motion: reduce) { .modebar { transition: none; } }
  @media (pointer: coarse) { .modes button, .modebar > button { min-height: 40px; } }
</style>
