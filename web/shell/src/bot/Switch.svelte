<svelte:options css="injected" />

<script lang="ts">
  // An on/off switch (`role="switch"`), named by `label` (shown beside it
  // unless `hideLabel`), described by `describedby`. Disabled, it stays in
  // the Tab order (`aria-disabled`) so its reason can be read; a press while
  // `busy` is ignored.
  let { checked, label, hideLabel = false, describedby, disabled = false, busy = false, onchange }: {
    checked: boolean; label: string; hideLabel?: boolean; describedby?: string; disabled?: boolean; busy?: boolean; onchange(on: boolean): void;
  } = $props();
</script>

<span class="bsw-wrap">
  <button type="button" role="switch" class="bsw" aria-checked={checked} aria-label={label} aria-describedby={describedby} aria-disabled={disabled || undefined} aria-busy={busy || undefined}
    onclick={() => { if (!disabled && !busy) onchange(!checked); }}><span></span></button>
  {#if !hideLabel}<span class="bsw-l" aria-hidden="true">{label}</span>{/if}
</span>

<style>
  :global {
    .bsw-wrap { display: inline-flex; align-items: center; gap: 10px; min-width: 0; }
    .bsw-l { font-weight: 600; font-size: 14px; overflow-wrap: anywhere; }
    .bsw { width: 40px; min-height: 24px; height: 24px; padding: 2px; border-radius: 999px; justify-content: flex-start; background: var(--border-strong); border-color: transparent; flex: none; }
    .bsw span { width: 18px; height: 18px; border-radius: 50%; background: var(--card); box-shadow: var(--elev); transition: transform var(--t); }
    .bsw[aria-checked="true"] { background: var(--accent); }
    .bsw[aria-checked="true"] span { transform: translateX(16px); }
    .bsw[aria-disabled="true"] { opacity: .5; cursor: default; }
    .bsw:not([aria-disabled="true"]):hover { background-color: var(--muted); border-color: transparent; }
    .bsw[aria-checked="true"]:not([aria-disabled="true"]):hover { background: var(--accent-hover); }
    @media (pointer: coarse) { .bsw-wrap { min-height: 44px; } }
  }
</style>
