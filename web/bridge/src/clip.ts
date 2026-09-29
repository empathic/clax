// Screenshot clips of the anchored region (spec §9 "Clips"), rendered in the
// frame with modern-screenshot. Images from other origins are left out (they
// would taint the canvas); in an opaque-origin sandbox every fetched image is
// cross-origin to the page, so those clips carry placeholders for images.
// modern-screenshot reads browser default styles from a helper `srcdoc` iframe;
// in an opaque-origin sandbox that iframe inherits the sandbox and cannot be
// read, so there it gets a never-attached iframe instead and every computed
// style is inlined.
//
// A clip is taken whenever the region it renders fits `MAX_CLIP_REGION`: an
// element, or a range's nearest block ancestor, that fits is rendered whole;
// a range in a larger block is rendered as a region around it (the lines
// within `REGION_PAD` above and below it, at the block's width) from a copy of
// just those lines, so rendering never walks the rest of the block. An
// element larger than the budget gets no clip.

import { createContext, destroyContext, domToPng } from "modern-screenshot";
import { type TextWindow, nextText, pointAt, prevText, windowAround } from "./text-walk";

export const MAX_SIDE = 1600;
export const CLIP_TIMEOUT_MS = 4000;
/** The largest region, in CSS px, a clip renders. */
export const MAX_CLIP_REGION = { w: 1600, h: 2400 };
/** How far above and below a range its region clip reaches, in CSS px. */
export const REGION_PAD = 120;

type Rect = { top: number; bottom: number; width: number; height: number };

/** Whether a region of `r`'s size fits the clip budget. */
export function fitsClipBudget(r: { width: number; height: number }): boolean {
  return r.width <= MAX_CLIP_REGION.w && r.height <= MAX_CLIP_REGION.h;
}

/** The vertical extent of the region clip for a range at `target` inside
 * `block` (client coordinates): `REGION_PAD` above and below the range, within
 * the block, at most `MAX_CLIP_REGION.h` tall (cut at the bottom). */
export function regionBounds(target: Rect, block: Rect): { top: number; bottom: number } {
  const top = Math.max(block.top, target.top - REGION_PAD);
  const bottom = Math.min(block.bottom, target.bottom + REGION_PAD, top + MAX_CLIP_REGION.h);
  return { top, bottom: Math.max(top, bottom) };
}
const TRANSPARENT = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/** Inline style for the cloned root. modern-screenshot drops the root's
 * computed margins, which lets the browser's default margins (1em on headings
 * and paragraphs) return inside the snapshot and push the content out of the
 * element-sized picture, so the root gets zero margins. When descendants' top
 * margins collapse through the root's top edge (they sit outside its box on
 * the page), the root becomes a block formatting context, which keeps those
 * margins inside it, and is pulled up by their collapsed size. */
export function clipRootStyle(el: Element, win: Window): Partial<CSSStyleDeclaration> {
  const m = collapsedTopMargin(el, win);
  return m ? { margin: "0", display: "flow-root", marginTop: `${-m}px` } : { margin: "0" };
}

/** Whether `el`'s top edge lets a first child's top margin collapse through it. */
function passesTopMargin(cs: CSSStyleDeclaration): boolean {
  return (cs.display === "block" || cs.display === "list-item")
    && !parseFloat(cs.borderTopWidth) && !parseFloat(cs.paddingTop)
    && (cs.overflow === "" || cs.overflow === "visible")
    && (cs.float === "" || cs.float === "none")
    && cs.position !== "absolute" && cs.position !== "fixed";
}

/** The first child of `el` in flow, or null when content other than an element
 * (non-blank text) comes first. */
function firstInFlowChild(el: Element, win: Window): Element | null {
  for (let n = el.firstChild; n; n = n.nextSibling) {
    if (n.nodeType === Node.TEXT_NODE) { if (/\S/.test(n.nodeValue ?? "")) return null; continue; }
    if (n.nodeType !== Node.ELEMENT_NODE) continue;
    const cs = win.getComputedStyle(n as Element);
    if (cs.display === "none" || cs.position === "absolute" || cs.position === "fixed") continue;
    return n as Element;
  }
  return null;
}

/** The size of the descendants' top margins that collapse through `el`'s top
 * edge (largest positive plus most negative, as CSS combines them). */
function collapsedTopMargin(el: Element, win: Window): number {
  let pos = 0;
  let neg = 0;
  let cur = el;
  while (passesTopMargin(win.getComputedStyle(cur))) {
    const child = firstInFlowChild(cur, win);
    if (!child) break;
    const cs = win.getComputedStyle(child);
    if (cs.display === "" || cs.display.startsWith("inline") || cs.display === "contents") break;
    const m = parseFloat(cs.marginTop) || 0;
    pos = Math.max(pos, m);
    neg = Math.min(neg, m);
    if (cs.display !== "block" && cs.display !== "list-item") break;
    cur = child;
  }
  return pos + neg;
}

const transparent = (c: string) => c === "" || c === "transparent" || /[,/]\s*0(\.0*)?\s*\)$/.test(c);

/** The colour to paint behind a clip of `el`: the nearest background colour
 * that is not fully transparent on `el` or its ancestors up to `<html>`, else
 * the page canvas for its `color-scheme` (white, or near-black when the page
 * renders dark), so clips never show transparent text on a transparent ground. */
export function clipBackground(el: Element, win: Window): string {
  for (let cur: Element | null = el; cur; cur = cur.parentElement) {
    const c = win.getComputedStyle(cur).backgroundColor;
    if (!transparent(c)) return c;
  }
  const scheme = win.getComputedStyle(el.ownerDocument.documentElement).colorScheme ?? "";
  const dark = /\bdark\b/.test(scheme) && (!/\blight\b/.test(scheme) || !!win.matchMedia?.("(prefers-color-scheme: dark)").matches);
  return dark ? "#121212" : "#ffffff";
}

/** Device pixel ratio, lowered so the long side stays within 1600 px. */
export function clipScale(w: number, h: number, dpr: number): number {
  const side = Math.max(w, h);
  return side > 0 ? Math.min(dpr, MAX_SIDE / side) : dpr;
}

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

export function crossOriginImage(n: Node, pageOrigin: string): boolean {
  if (!(n instanceof HTMLImageElement)) return false;
  try {
    const u = new URL(n.currentSrc || n.src, n.baseURI);
    return u.protocol !== "data:" && u.protocol !== "blob:" && u.origin !== pageOrigin;
  } catch {
    return false;
  }
}

export function dataUrlToBuffer(url: string): ArrayBuffer {
  const bin = atob(url.slice(url.indexOf(",") + 1));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

/** A PNG of `el`; rejects when it has no size or rendering exceeds `timeoutMs`. */
export async function renderClip(el: Element, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) throw new Error("the anchored element has no size");
  const origin = new URL(win.location.href).origin;
  const png = (async () => {
    const ctx = await createContext(el, {
      scale: clipScale(r.width, r.height, win.devicePixelRatio || 1),
      filter: n => !crossOriginImage(n, origin),
      fetch: { placeholderImage: TRANSPARENT },
      backgroundColor: clipBackground(el, win),
      style: clipRootStyle(el, win),
      timeout: timeoutMs,
    });
    if (win.origin === "null") ctx.sandbox = el.ownerDocument.createElement("iframe");
    try {
      return await domToPng(ctx);
    } finally {
      destroyContext(ctx);
    }
  })();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`clip took longer than ${timeoutMs} ms`)), timeoutMs); });
  try {
    return dataUrlToBuffer(await Promise.race([png, late]));
  } finally {
    clearTimeout(timer);
  }
}

/** A PNG of `t` within the clip budget: an element that fits it, or a range's
 * nearest block ancestor when that fits, is rendered whole; a range in a
 * larger block is rendered as the region around it (`renderRegion`). Rejects
 * for an element larger than the budget. */
export async function renderTargetClip(t: Element | Range, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  if ("nodeType" in t) {
    if (!fitsClipBudget(t.getBoundingClientRect())) throw new Error("the element is too large to capture");
    return renderClip(t, win, timeoutMs);
  }
  const range = t;
  const block = blockAncestor(range.commonAncestorContainer, win);
  if (fitsClipBudget(block.getBoundingClientRect())) return renderClip(block, win, timeoutMs);
  return renderRegion(block, range, win, timeoutMs);
}

/** The text point at or after (`c`, `o`) in `root`. */
function textAfter(root: Node, c: Node, o: number): { node: Text; offset: number } | null {
  if (c.nodeType === Node.TEXT_NODE) return { node: c as Text, offset: o };
  const child = c.childNodes[o];
  if (child?.nodeType === Node.TEXT_NODE) return { node: child as Text, offset: 0 };
  let last: Node = c;
  if (!child) while (last.lastChild) last = last.lastChild;
  const t = nextText(root, child ?? last);
  return t ? { node: t, offset: 0 } : null;
}

/** The text point at or before (`c`, `o`) in `root`. */
function textBefore(root: Node, c: Node, o: number): { node: Text; offset: number } | null {
  if (c.nodeType === Node.TEXT_NODE) return { node: c as Text, offset: o };
  let n: Node | null = o > 0 ? c.childNodes[o - 1] : null;
  if (n) while (n.lastChild) n = n.lastChild;
  if (n?.nodeType === Node.TEXT_NODE) return { node: n as Text, offset: (n as Text).length };
  const t = prevText(root, n ?? c);
  return t ? { node: t, offset: t.length } : null;
}

const NEVER = /(?!)/g;

/** The rectangle of the character at `i` in `w`, or of the first one within
 * eight characters of it (towards `step`) that has one. */
function charRect(w: TextWindow, i: number, step: 1 | -1): DOMRect | null {
  const doc = w.segs[0].node.ownerDocument;
  const r = doc.createRange();
  for (let k = 0; k < 8; k++, i += step) {
    if (i < 0 || i >= w.text.length) return null;
    const p = pointAt(w, i, false);
    r.setStart(p.node, p.offset);
    r.setEnd(p.node, p.offset + 1);
    const b = typeof r.getBoundingClientRect === "function" ? r.getBoundingClientRect() : null;
    if (b && (b.width || b.height)) return b;
  }
  return null;
}

/** The first offset in `w` at or below `w.at` whose character ends below `top`
 * (the start of the first visual line in the region). */
function regionStart(w: TextWindow, top: number): number {
  let lo = 0;
  let hi = w.at;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const b = charRect(w, mid, 1);
    if (!b || b.bottom > top) hi = mid; else lo = mid + 1;
  }
  return w.text[lo] === "\n" ? lo + 1 : lo;
}

/** The offset in `w`, at or after `w.at`, just past the last character that
 * starts above `bottom` (the end of the last visual line in the region). */
function regionEnd(w: TextWindow, bottom: number): number {
  let lo = w.at;
  let hi = w.text.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const b = charRect(w, mid, -1);
    if (!b || b.top < bottom) lo = mid + 1; else hi = mid;
  }
  return lo > w.at && w.text[lo - 1] === "\n" ? lo - 1 : lo;
}

/** A PNG of the region around `range` in `block` (see `regionBounds`), at the
 * block's width. The lines in the region are copied into a shallow copy of
 * the block (with shallow copies of the elements between them, so the page's
 * styles still apply), placed next to it off-screen, rendered, and removed. */
export async function renderRegion(block: Element, range: Range, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  const doc = block.ownerDocument;
  const br = block.getBoundingClientRect();
  const { top, bottom } = regionBounds(range.getBoundingClientRect(), br);
  const s = textAfter(block, range.startContainer, range.startOffset);
  const e = textBefore(block, range.endContainer, range.endOffset);
  const part = doc.createRange();
  if (s && e) {
    const above = windowAround(block, s.node, s.offset, NEVER);
    const below = windowAround(block, e.node, e.offset, NEVER);
    const a = pointAt(above, regionStart(above, top), false);
    const b = pointAt(below, regionEnd(below, bottom), true);
    part.setStart(a.node, a.offset);
    part.setEnd(b.node, b.offset);
    if (part.collapsed) { part.setStart(range.startContainer, range.startOffset); part.setEnd(range.endContainer, range.endOffset); }
  } else {
    part.setStart(range.startContainer, range.startOffset);
    part.setEnd(range.endContainer, range.endOffset);
  }
  let inner: Node = part.cloneContents();
  const common = part.commonAncestorContainer;
  for (let a: Element | null = common.nodeType === Node.ELEMENT_NODE ? (common as Element) : common.parentElement; a && a !== block && block.contains(a); a = a.parentElement) {
    const copy = a.cloneNode(false);
    copy.appendChild(inner);
    inner = copy;
  }
  const copy = block.cloneNode(false) as HTMLElement;
  copy.appendChild(inner);
  const fixed: Record<string, string> = {
    position: "fixed", left: "-100000px", top: "0", width: `${br.width}px`, "box-sizing": "border-box",
    height: "auto", "min-height": "0", "max-height": "none", margin: "0", overflow: "hidden",
    transform: "none", "pointer-events": "none",
  };
  for (const [k, v] of Object.entries(fixed)) copy.style?.setProperty(k, v, "important");
  copy.setAttribute("aria-hidden", "true");
  block.after(copy);
  try {
    return await renderClip(copy, win, timeoutMs);
  } finally {
    copy.remove();
  }
}
