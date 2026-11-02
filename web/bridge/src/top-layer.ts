// Keeps an overlay's host above what the page puts in the top layer. A
// page's modal dialog (`showModal()`) goes in the top layer, over any
// z-index, and makes everything outside it inert: an overlay that is a child
// of <html> then draws under the dialog and takes no pointer. The host is a
// manual popover (top layer too), placed inside the page's open modal dialog
// while there is one (so it is not inert; a top-layer element is positioned
// against the viewport wherever it sits), else a child of <html>, and shown
// again whenever a dialog opens or closes, which puts it last in the top
// layer, above the dialog.

/** The page's open modal dialog, the last in document order when there are
 * several; null when there is none. */
export function topModal(doc: Document): Element | null {
  let top: Element | null = null;
  for (const d of doc.querySelectorAll("dialog[open]")) {
    try { if (d.matches(":modal")) top = d; } catch { /* no :modal: no top layer */ }
  }
  return top;
}

/** Keeps `host` (with `popover="manual"`) in the page's open modal dialog
 * or in <html>, and last in the top layer, until `stop` runs; `placed`
 * hears each move. A dialog removed with the host in it puts the host back.
 * `place` puts it where it belongs now, and `parent` says where that is.
 *
 * Moving a connected node by re-inserting it reloads every iframe in it,
 * and an open composer is an iframe: a page toggling any `open` (a
 * <details>, a menu, its own panels) used to reload it and lose the pick.
 * So the host moves only when its parent must change, with `moveBefore`
 * (which keeps iframes) where the browser has it; without it, never while
 * `holding()` says an iframe in the host must not reload. The top layer
 * orders by showing, not by document order, so being the parent's last
 * child is not needed. */
export function keepOnTop(
  doc: Document,
  host: HTMLElement,
  placed: () => void = () => {},
  holding: () => boolean = () => false,
) {
  let live = true;
  let inDialog = false;
  let shownOver: Element | null | undefined;
  // The last `place` left the host where it was so as not to reload it.
  let held = false;
  const parent = () => topModal(doc) ?? doc.documentElement;
  const place = () => {
    if (!live) return;
    const p = parent();
    const modal = p === doc.documentElement ? null : p;
    let moved = false;
    held = false;
    if (host.parentNode !== p) {
      const into = p as Element & { moveBefore?: (node: Node, child: Node | null) => void };
      if (host.isConnected && typeof into.moveBefore === "function") {
        try { into.moveBefore(host, null); moved = true; } catch { /* not movable: handled below */ }
      }
      if (!moved && (!host.isConnected || !holding())) { p.appendChild(host); moved = true; }
      held = !moved;
    }
    inDialog = host.parentNode !== doc.documentElement;
    // Shown again, it is last in the top layer: only when it moved, a
    // different modal is on top, or it is not showing, so a toggle that
    // changes neither does not take focus out of the composer.
    try {
      const showing = host.matches(":popover-open");
      if (moved || !showing || shownOver !== modal) {
        if (showing) host.hidePopover();
        host.showPopover();
      }
    } catch { /* no popovers */ }
    shownOver = modal;
    if (moved) placed();
  };
  const watch = new MutationObserver(records => {
    if (records.some(r => r.attributeName === "open") || (inDialog && !host.isConnected)) place();
  });
  watch.observe(doc, { subtree: true, childList: true, attributes: true, attributeFilter: ["open"] });
  return { stop: () => { live = false; watch.disconnect(); }, parent, place, held: () => held };
}
