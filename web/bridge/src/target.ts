// What comment mode targets under the pointer. An element that fits the
// viewport is the target. An oversized one (a whole file in one <pre>, a long
// article in one <div>) would be outlined off-screen and anchored as a whole,
// so the text under the pointer is targeted instead: its line in
// preformatted text, else the block around it, else its sentence.

import { boundaryOffset, OVERLAY_TAG, rangeAt, textIndex } from "./anchor";

/** An element covering more than this share of the viewport's area is oversized
 * (as is one taller or wider than the viewport). */
export const OVERSIZED_SHARE = 0.6;
/** Inset of an outline clamped to the viewport, so all four borders show. */
const INSET = 2;

export interface Viewport { w: number; h: number }
type Rect = { left: number; top: number; right: number; bottom: number; width: number; height: number };

/** `t`'s bounding client rectangle (a Range without layout measures as empty). */
export function rectOf(t: Element | Range): DOMRect {
  return typeof t.getBoundingClientRect === "function" ? t.getBoundingClientRect() : new DOMRect(0, 0, 0, 0);
}

/** The viewport without scrollbars. */
export function viewportOf(doc: Document): Viewport {
  const win = doc.defaultView!;
  return { w: doc.documentElement.clientWidth || win.innerWidth, h: doc.documentElement.clientHeight || win.innerHeight };
}

export function isOversized(r: Rect, vp: Viewport): boolean {
  if (r.height > vp.h || r.width > vp.w) return true;
  const w = Math.max(0, Math.min(r.right, vp.w) - Math.max(r.left, 0));
  const h = Math.max(0, Math.min(r.bottom, vp.h) - Math.max(r.top, 0));
  return w * h > OVERSIZED_SHARE * vp.w * vp.h;
}

/** The outline around `r` (2 px outside it), clamped to the viewport and inset
 * by 2 px on each clamped side so all four borders show (at least 4 px each
 * way); null when no part of it is in view. */
export function outlineBox(r: Rect, vp: Viewport): { left: number; top: number; width: number; height: number } | null {
  if (r.bottom < 0 || r.top > vp.h || r.right < 0 || r.left > vp.w) return null;
  let left = r.left - INSET;
  let top = r.top - INSET;
  let right = r.right + INSET;
  let bottom = r.bottom + INSET;
  if (left < 0) left = INSET;
  if (top < 0) top = INSET;
  if (right > vp.w) right = vp.w - INSET;
  if (bottom > vp.h) bottom = vp.h - INSET;
  // A region clamped (or measured) to almost nothing still shows its borders.
  right = Math.max(right, left + 2 * INSET);
  bottom = Math.max(bottom, top + 2 * INSET);
  return { left, top, width: right - left, height: bottom - top };
}

function luminance([r, g, b]: number[]): number {
  const lin = (v: number) => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** Outline colours for a page whose background is the CSS colour `bg`
 * (transparent counts as white): a border with at least 3:1 contrast on it and
 * a tint of at least 16%. */
export function outlineColors(bg: string): { border: string; tint: string; tintAlpha: number } {
  const m = bg.match(/rgba?\(([^)]+)\)/);
  const parts = m ? m[1].split(",").map(s => parseFloat(s)) : [255, 255, 255, 1];
  const alpha = parts.length > 3 ? parts[3] : 1;
  const rgb = alpha === 0 ? [255, 255, 255] : parts.slice(0, 3);
  return luminance(rgb) < 0.4
    ? { border: "#fdba74", tint: "rgba(253, 186, 116, 0.2)", tintAlpha: 0.2 }
    : { border: "#c2410c", tint: "rgba(194, 65, 12, 0.18)", tintAlpha: 0.18 };
}

/** The page's background colour: `body`'s, else the root's, else transparent. */
export function pageBackground(doc: Document): string {
  const win = doc.defaultView!;
  for (const el of [doc.body, doc.documentElement]) {
    if (!el) continue;
    const c = win.getComputedStyle(el).backgroundColor;
    if (c && c !== "transparent" && !/rgba\([^)]*,\s*0\)$/.test(c)) return c;
  }
  return "rgba(0, 0, 0, 0)";
}

/** The text position under (`x`, `y`), from `caretPositionFromPoint` or `caretRangeFromPoint`. */
export function caretAt(doc: Document, x: number, y: number): { node: Node; offset: number } | null {
  const d = doc as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  if (typeof d.caretPositionFromPoint === "function") {
    const p = d.caretPositionFromPoint(x, y);
    return p ? { node: p.offsetNode, offset: p.offset } : null;
  }
  if (typeof d.caretRangeFromPoint === "function") {
    const r = d.caretRangeFromPoint(x, y);
    return r ? { node: r.startContainer, offset: r.startOffset } : null;
  }
  return null;
}

const isInline = (el: Element) => (el.ownerDocument.defaultView!.getComputedStyle(el).display || "inline").startsWith("inline");
const isPre = (el: Element) => /^(pre|break-spaces)/.test(el.ownerDocument.defaultView!.getComputedStyle(el).whiteSpace || "") || el.localName === "pre";

/** A range over `text` from `start` to `end` in `block`, narrowed to exclude
 * surrounding whitespace; null when only whitespace is left. */
function trimmedRange(block: Element, start: number, end: number): Range | null {
  const idx = textIndex(block);
  while (start < end && /\s/.test(idx.text[start])) start++;
  while (end > start && /\s/.test(idx.text[end - 1])) end--;
  return end > start ? rangeAt(idx, start, end) : null;
}

/** What comment mode targets for the pointer at (`x`, `y`) over `el`: `el`
 * itself when it fits the viewport; otherwise the line under the pointer in
 * preformatted text, the block around the text when that block fits, or the
 * sentence around it; with no text under the pointer, the smallest element
 * under it that fits; else `el`. */
export function chooseTarget(doc: Document, el: Element, x: number, y: number, vp: Viewport = viewportOf(doc)): Element | Range {
  if (!isOversized(el.getBoundingClientRect(), vp)) return el;
  const caret = caretAt(doc, x, y);
  const text = caret && caret.node.nodeType === Node.TEXT_NODE && el.contains(caret.node) ? (caret.node as Text) : null;
  const parent = text?.parentElement;
  if (text && parent && !parent.closest(OVERLAY_TAG) && !/^(script|style|noscript|template)$/.test(parent.localName)) {
    let block: Element = parent;
    while (block !== el && isInline(block) && block.parentElement) block = block.parentElement;
    if (isPre(parent) || isPre(block)) {
      const idx = textIndex(block);
      const at = boundaryOffset(idx, text, caret!.offset);
      const start = idx.text.lastIndexOf("\n", at - 1) + 1;
      const next = idx.text.indexOf("\n", at);
      const line = trimmedRange(block, start, next < 0 ? idx.text.length : next);
      if (line) return line;
    } else if (block !== el && !isOversized(block.getBoundingClientRect(), vp)) {
      return block;
    } else {
      const idx = textIndex(block);
      const at = boundaryOffset(idx, text, caret!.offset);
      let start = 0;
      for (const m of idx.text.slice(0, at).matchAll(/[.!?](?=\s)|\n/g)) start = m.index + 1;
      const rest = idx.text.slice(at).match(/[.!?](?=\s|$)|\n/);
      const end = rest?.index !== undefined ? at + rest.index + (rest[0] === "\n" ? 0 : 1) : idx.text.length;
      const sentence = trimmedRange(block, start, end);
      if (sentence) return sentence;
    }
  }
  const under = typeof doc.elementsFromPoint === "function" ? doc.elementsFromPoint(x, y) : [];
  for (const e of under) {
    if (e !== el && el.contains(e) && !e.closest(OVERLAY_TAG) && !isOversized(e.getBoundingClientRect(), vp)) return e;
  }
  return el;
}
