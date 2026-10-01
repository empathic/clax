// Drawn areas and Option widening in comment mode (spec §8 "Comment mode").
// A drag that starts where there is no text under the pointer (empty space,
// padding, an image or canvas), or any drag with Shift held, draws a
// rectangle; its anchor names the smallest element whose border box holds the
// rectangle (an inline `<svg>` or other foreign content counts as one element)
// and places the rectangle in it as fractions, so it follows that element on
// later versions; a rectangle no element holds is placed on the document
// (`html`, its whole scrollable size). Holding Option (Alt) targets the
// enclosing element of the hovered target, one ancestor more per Up press.

import { OVERLAY_TAG, cssPath, fingerprint, htmlHash } from "./anchor";
import { blockAncestor } from "./block";
import { type Anchor, type AnchorArea, type Box, INDEX_FILE } from "./protocol";
import { caretAt } from "./target";
import { readable } from "./text-walk";

/** A drag rectangle narrower or shorter than this, in CSS px, is a click. */
export const AREA_MIN = 8;
/** How far from the pointer, in CSS px, the text a caret lands on may be and
 * still count as text under the pointer. */
export const TEXT_NEAR = 4;
/** Replaced and graphic elements: a drag over one draws an area. */
export const REPLACED = "img, svg, canvas, video, iframe, object, embed, picture";

/** A rectangle in the frame's viewport, in CSS px. */
export interface AreaRect { left: number; top: number; width: number; height: number }
type Point = { x: number; y: number };

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
/** Fractions keep 6 decimal places: under 1 px on elements up to 1,000,000 px. */
const r6 = (v: number) => Math.round(v * 1e6) / 1e6;
const XHTML = "http://www.w3.org/1999/xhtml";

/** The rectangle between the drag's start `a` and its current point `b`,
 * both clamped to the viewport `vp`. */
export function dragRect(a: Point, b: Point, vp: { w: number; h: number }): AreaRect {
  const ax = clamp(a.x, 0, vp.w);
  const ay = clamp(a.y, 0, vp.h);
  const bx = clamp(b.x, 0, vp.w);
  const by = clamp(b.y, 0, vp.h);
  return { left: Math.min(ax, bx), top: Math.min(ay, by), width: Math.abs(bx - ax), height: Math.abs(by - ay) };
}

/** Whether a drag rectangle is too small to be an area (a click): narrower
 * or shorter than `AREA_MIN`. */
export function isClickSized(r: AreaRect): boolean {
  return r.width < AREA_MIN || r.height < AREA_MIN;
}

/** `r` as fractions of `box` (both in client coordinates), rounded to 6
 * decimal places and clamped into the box, with some width and height. */
export function areaFractions(r: AreaRect, box: AreaRect): AnchorArea {
  const fx = (v: number) => (box.width > 0 ? clamp((v - box.left) / box.width, 0, 1) : 0);
  const fy = (v: number) => (box.height > 0 ? clamp((v - box.top) / box.height, 0, 1) : 0);
  // A rectangle clamped onto the box's far edge keeps a sliver of width.
  const x = Math.min(r6(fx(r.left)), 0.999999);
  const y = Math.min(r6(fy(r.top)), 0.999999);
  const w = Math.min(Math.max(1e-6, r6(fx(r.left + r.width) - fx(r.left))), r6(1 - x));
  const h = Math.min(Math.max(1e-6, r6(fy(r.top + r.height) - fy(r.top))), r6(1 - y));
  return { x, y, w, h };
}

/** Where area anchor `a`, resolved to `el`, is now (client coordinates): on
 * the document (`html`) its drawn rectangle at the same page coordinates
 * (`rect` plus the scroll at draw time), so a resize or a longer page does
 * not move it; on any other element its fractions projected onto the
 * element's box. */
export function placeArea(a: Anchor, el: Element): Box {
  const doc = el.ownerDocument;
  if (el === doc.documentElement && a.rect) {
    const win = doc.defaultView!;
    return { x: a.rect.x + a.rect.scrollX - win.scrollX, y: a.rect.y + a.rect.scrollY - win.scrollY, w: a.rect.w, h: a.rect.h };
  }
  return areaBox(a.area!, boxOf(el));
}

/** `area` projected onto an element's border box `box` (client coordinates). */
export function areaBox(area: AnchorArea, box: AreaRect): Box {
  return { x: box.left + area.x * box.width, y: box.top + area.y * box.height, w: area.w * box.width, h: area.h * box.height };
}

const holds = (b: AreaRect, r: AreaRect) =>
  b.left <= r.left + 0.5 && b.top <= r.top + 0.5 && b.left + b.width >= r.left + r.width - 0.5 && b.top + b.height >= r.top + r.height - 0.5;

/** The box an area is placed in: for `<html>` (the document), the whole
 * scrollable page in client coordinates; for any other element, its border box. */
export function boxOf(el: Element): AreaRect {
  const doc = el.ownerDocument;
  if (el === doc.documentElement) {
    const win = doc.defaultView!;
    const se = doc.scrollingElement ?? doc.documentElement;
    return { left: -win.scrollX, top: -win.scrollY, width: Math.max(se.scrollWidth, se.clientWidth), height: Math.max(se.scrollHeight, se.clientHeight) };
  }
  const b = el.getBoundingClientRect();
  return { left: b.left, top: b.top, width: b.width, height: b.height };
}

/** `el`, or for an element inside foreign content (inline SVG, MathML) the
 * outermost foreign element around it: the `<svg>` of a chart, never one of
 * its shapes. */
export function foreignRoot(el: Element): Element {
  let cur = el;
  while (cur.namespaceURI !== XHTML && cur.parentElement && cur.parentElement.namespaceURI !== XHTML) cur = cur.parentElement;
  return cur;
}

/** The smallest element under the rectangle's centre, or an ancestor of one,
 * whose border box holds all of `r`, counting inline SVG and other foreign
 * content as one element (its outermost foreign element); `<html>` (the
 * document) when no element in the body holds it. */
export function containingElement(doc: Document, r: AreaRect): Element {
  const hits = typeof doc.elementsFromPoint === "function" ? doc.elementsFromPoint(r.left + r.width / 2, r.top + r.height / 2) : [];
  let best: Element | null = null;
  let bestArea = Infinity;
  for (const hit of hits) {
    if (hit.closest(OVERLAY_TAG)) continue;
    // HTML inside an SVG `<foreignObject>` walks up to the `<svg>`, not into its shapes.
    for (let e: Element | null = foreignRoot(hit); e && e !== doc.documentElement; e = e.parentElement ? foreignRoot(e.parentElement) : null) {
      const b = e.getBoundingClientRect();
      if (!holds(b, r)) continue;
      if (b.width * b.height < bestArea) { best = e; bestArea = b.width * b.height; }
      break;
    }
  }
  return best ?? doc.documentElement;
}

/** An area anchor for the rectangle `r` (viewport pixels) drawn on the page
 * published at `file`, in `el` (its `containingElement` unless given). */
export function buildAreaAnchor(doc: Document, r: AreaRect, file: string = INDEX_FILE, el: Element = containingElement(doc, r)): Anchor {
  const win = doc.defaultView!;
  return {
    kind: "area", selector: el === doc.documentElement ? "html" : cssPath(el), quote: null, prefix: null, suffix: null, html_hash: el === doc.documentElement ? null : htmlHash(el),
    rect: { x: r.left, y: r.top, w: r.width, h: r.height, scrollX: win.scrollX, scrollY: win.scrollY, viewportW: win.innerWidth },
    custom_name: null, area: el === doc.documentElement ? areaFractions(r, boxOf(el)) : { ...areaFractions(r, boxOf(el)), ...fingerprint(el) }, file,
  };
}

/** Whether there is no text under the pointer at (`x`, `y`) over `el`: `el`
 * is (inside) a replaced or graphic element (`REPLACED`), or the caret there
 * is not in visible, non-blank text, or the characters around it lie more
 * than `TEXT_NEAR` px from the pointer (browsers put the caret on the nearest
 * text even far from it). */
export function nonTextAt(doc: Document, el: Element | null, x: number, y: number): boolean {
  if (el?.closest(REPLACED)) return true;
  const c = caretAt(doc, x, y);
  if (!c || c.node.nodeType !== Node.TEXT_NODE) return true;
  const t = c.node as Text;
  if (!readable(t) || !/\S/.test(t.data)) return true;
  const range = doc.createRange();
  range.setStart(t, Math.max(0, c.offset - 1));
  range.setEnd(t, Math.min(t.length, c.offset + 1));
  const rects = typeof range.getClientRects === "function" ? Array.from(range.getClientRects()) : [];
  return !rects.some(b => (b.width > 0 || b.height > 0)
    && x >= b.left - TEXT_NEAR && x <= b.right + TEXT_NEAR && y >= b.top - TEXT_NEAR && y <= b.bottom + TEXT_NEAR);
}

/** Option widening: while `active`, the target is `level` elements out from
 * the hovered one (see `widenedTarget`); level 0 is the hovered target. */
export class Widen {
  active = false;
  level = 0;
  /** Option went down: the enclosing element is the target. */
  start(): void { this.active = true; this.level = 1; }
  /** Option went up: the hovered target is the target again. */
  stop(): void { this.active = false; this.level = 0; }
  /** Up: one ancestor further out (held to what exists by `clamp`). */
  up(): void { if (this.active) this.level++; }
  /** Down: one ancestor back toward the hovered target. */
  down(): void { if (this.active && this.level > 0) this.level--; }
  /** Holds the level to the one `widenedTarget` reached. */
  clamp(level: number): void { if (this.active) this.level = level; }
}

/** The target `level` elements out from `base`: for a range (a line, a
 * sentence), level 1 is its nearest block ancestor; for an element, its
 * parent; each further level is one more ancestor, never past the body.
 * Returns the target and the level actually reached. */
export function widenedTarget(base: Element | Range, level: number): { target: Element | Range; level: number } {
  if (level <= 0) return { target: base, level: 0 };
  const isEl = "nodeType" in base;
  const doc = isEl ? base.ownerDocument : base.commonAncestorContainer.ownerDocument!;
  if (isEl && (base === doc.body || base === doc.documentElement || !base.parentElement)) return { target: base, level: 0 };
  let el: Element = isEl ? base.parentElement! : blockAncestor(base.commonAncestorContainer, doc.defaultView!);
  let n = 1;
  while (n < level && el !== doc.body && el.parentElement && el.parentElement !== doc.documentElement) {
    el = el.parentElement;
    n++;
  }
  return { target: el, level: n };
}
