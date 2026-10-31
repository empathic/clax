<script lang="ts">
  // A thread's pin over the page: its number in the pin colours, which are
  // the same in both themes (a pin sits on the page, whose colours Clax does
  // not set). `addressed` shows a thread an agent answered: the pin turns to
  // the ring's colour with the agent's edge. Placed by its caller (`left`,
  // `top` in the positioned parent's pixels). Styled only through Clax's
  // tokens.
  type Props = { n: number; label: string; left?: number; top?: number; addressed?: boolean; onclick?: (e: MouseEvent) => void };
  let { n, label, left, top, addressed = false, onclick }: Props = $props();
</script>

<button type="button" class={addressed ? "pin addressed" : "pin"} aria-label={label}
  style:left={left === undefined ? undefined : `${left}px`} style:top={top === undefined ? undefined : `${top}px`} {onclick}>{n}</button>

<style>
  .pin { position: absolute; display: inline-grid; place-items: center; width: 22px; height: 22px; min-height: 0; padding: 0; border-radius: 50%; background: var(--pin); color: var(--on-pin); border: 2px solid var(--pin-ring); box-shadow: 0 1px 3px var(--shadow); font: 600 11.5px/1 var(--font); font-variant-numeric: tabular-nums; cursor: pointer; }
  .pin:hover, .pin:focus-visible { border-color: var(--focus); outline: none; }
  .addressed { background: var(--pin-ring); color: var(--selection-fg); border-color: var(--agent-ink); }
</style>
