<script lang="ts">
  // A thread's pin over the page: its number in the pin colours, which are
  // the same in both themes (a pin sits on the page, whose colours Clax does
  // not set). Its states:
  // - `addressed`: an agent answered the thread; the pin turns to the ring's
  //   colour with the agent's edge, and `flag` (such as "v2") names the
  //   version that did it, beside the pin;
  // - `onit`: an agent is working on it; the pin is split, person and agent;
  // - `settling`: it just appeared or moved, and takes no press (a page that
  //   places it under the resting pointer cannot take the viewer's click).
  // Placed by its caller (`left`, `top` in the positioned parent's pixels).
  // The caller's class comes first. Styled only through Clax's tokens.
  type Props = {
    n: number;
    label: string;
    title?: string;
    left?: number;
    top?: number;
    addressed?: boolean;
    flag?: string;
    onit?: boolean;
    settling?: boolean;
    class?: string;
    onclick?: (e: MouseEvent) => void;
    onenter?: () => void;
    onleave?: () => void;
  };
  let { n, label, title, left, top, addressed = false, flag, onit = false, settling = false, class: cls = "", onclick, onenter, onleave }: Props = $props();
  const classes = $derived(`${cls} pin${addressed ? " addressed" : ""}${onit ? " onit" : ""}${settling ? " settling" : ""}`.trim());
</script>

<button type="button" class={classes} aria-label={label} {title} data-v={flag}
  style:left={left === undefined ? undefined : `${left}px`} style:top={top === undefined ? undefined : `${top}px`}
  {onclick} onmouseenter={onenter} onmouseleave={onleave}>{n}</button>

<style>
  .pin { position: absolute; pointer-events: auto; display: inline-grid; place-items: center; flex: none; width: 22px; height: 22px; min-height: 0; margin: 0; padding: 0; border-radius: 50%; background: var(--pin); color: var(--on-pin); border: 2px solid var(--pin-ring); box-shadow: 0 1px 3px var(--shadow); font: 600 11.5px/1 var(--font); font-variant-numeric: tabular-nums; cursor: pointer; }
  .pin:hover { background-color: var(--pin); border-color: var(--focus); }
  .pin:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
  .pin.addressed, .pin.addressed:hover { background-color: var(--pin-ring); color: var(--pin-ink); border-color: var(--pin-agent); }
  .pin.onit, .pin.onit:hover { background: linear-gradient(90deg, var(--pin) 50%, var(--pin-agent) 50%); color: var(--pin-ring); text-shadow: 0 0 2px var(--on-pin); }
  .pin.settling { pointer-events: none; }
  .pin[data-v]::after { content: attr(data-v); position: absolute; left: 22px; top: 2px; font: 600 10.5px/14px var(--font); background: var(--pin-ring); color: var(--pin-ink); border: 1px solid var(--pin-agent); border-radius: var(--radius-xs); padding: 0 4px; white-space: nowrap; }
</style>
