// The nearest block-level ancestor of a node: shared by comment mode (hover
// and widening) and clip rendering, so comment mode does not pull in clip.ts.

/** Whether `el` generates no block-level box of its own (inline, `contents`,
 * or no computed display, which is how jsdom reports inline elements). */
function inlineLike(el: Element, win: Window): boolean {
  const d = win.getComputedStyle(el).display;
  return d === "" || d === "contents" || d.startsWith("inline");
}

/** `node`'s element, or its nearest ancestor that is not inline. */
export function blockAncestor(node: Node, win: Window): Element {
  let el = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement!;
  while (el.parentElement && el !== el.ownerDocument.body && inlineLike(el, win)) el = el.parentElement;
  return el;
}
