// The artifact view's top bar renders its controls at once, in their final
// places, as soon as the view is ready and their code is in
// (TopbarIsland.svelte): no control waits on another's request. An
// optional control (Inbox, Fork, Share, Publish, Get in Clax, the feedback
// menu, ClaxBot's button) sits in a slot (`late`, on an element
// of `display: contents`): a slot holding its control when the bar first
// shows keeps the control in its place; one whose control comes after that
// (its answer was not in: `view/deciders.svelte.ts`) puts it, by CSS
// `order` (`--late`), beside what gives way to it, so its arrival moves no
// other control: right after the bar's free room (`.bar-gap`, between the
// title and the controls) while that room, or the title, gives way to it,
// else right after the people pill (which narrows, keeping its place),
// each later one before the one before it. The bar's order (TopbarIsland.svelte): the mark, the title, the
// free room, the controls that came late beside it, Inbox, the call count
// and the pill (`PILL_ORDER`), the controls that came late beside the pill,
// then the rest. The visual order then differs from the DOM's for a late
// control alone.

let shown = $state(false);

/** The order of Inbox, the call count and the people pill (theme.css);
 * the free room's is `GAP_ORDER`. */
export const PILL_ORDER = -50;
export const GAP_ORDER = -100;
let byGap = PILL_ORDER;
let byPill = 0;

/** Whether the bar has shown its controls (its first render, painted). */
export const bar = { get shown(): boolean { return shown; } };

/** Marks the bar's controls shown: a slot filled from now on is late. */
export function markShown(): void { shown = true; }

/** The CSS `order` the next late control takes: right after the free room
 * (`room`: it has room for the control), else right after the pill;
 * before every late control there already. */
export function lateOrder(room: boolean): number {
  return room ? Math.max(GAP_ORDER + 1, --byGap) : Math.max(PILL_ORDER + 1, --byPill);
}

/** Whether the bar `node` is in gives room beside its free room: the room
 * is open, or the title is wider than its least width (both give way
 * before the pill). */
function gapRoom(node: Element): boolean {
  const top = node.closest(".topbar");
  const gap = top?.querySelector(".bar-gap");
  const ttl = top?.querySelector<HTMLElement>(":scope > .ttl");
  if ((gap?.getBoundingClientRect().width ?? 0) > 0.5) return true;
  return !!ttl && ttl.getBoundingClientRect().width - (parseFloat(getComputedStyle(ttl).minWidth) || 0) > 0.5;
}

/** A slot's action: when `node` first holds an element, it is in its place
 * if the bar has not shown yet, else late (`--late`). Decided once, as its
 * control is laid out where the DOM puts it (the free room has given it
 * its width already, if it had it). */
export function late(node: HTMLElement): { destroy(): void } {
  let done = false;
  const check = () => {
    if (done || !node.firstElementChild) return;
    done = true;
    obs.disconnect();
    if (shown) node.style.setProperty("--late", String(lateOrder(gapRoom(node))));
  };
  const obs = new MutationObserver(check);
  obs.observe(node, { childList: true });
  check();
  return { destroy: () => obs.disconnect() };
}
