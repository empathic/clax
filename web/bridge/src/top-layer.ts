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
 * or in <html>, last there and last in the top layer, until `stop` runs;
 * `placed` hears each move. A dialog removed with the host in it puts the
 * host back. `place` puts it where it belongs now, and `parent` says where
 * that is. */
export function keepOnTop(doc: Document, host: HTMLElement, placed: () => void = () => {}) {
  let live = true;
  let inDialog = false;
  const parent = () => topModal(doc) ?? doc.documentElement;
  const place = () => {
    if (!live) return;
    const p = parent();
    if (host.parentNode !== p || p.lastElementChild !== host) p.appendChild(host);
    inDialog = p !== doc.documentElement;
    // Shown again, it is last in the top layer.
    try { if (host.matches(":popover-open")) host.hidePopover(); host.showPopover(); } catch { /* no popovers */ }
    placed();
  };
  const watch = new MutationObserver(records => {
    if (records.some(r => r.attributeName === "open") || (inDialog && !host.isConnected)) place();
  });
  watch.observe(doc, { subtree: true, childList: true, attributes: true, attributeFilter: ["open"] });
  return { stop: () => { live = false; watch.disconnect(); }, parent, place };
}
