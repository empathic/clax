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
// just those lines, with the range marked, so rendering never walks the rest
// of the block; the copy is made inert before it is connected
// (`neutraliseCopy`). An element larger than the budget is rendered cropped
// to its part in the viewport, grown to the budget within it, on each axis
// over the budget that is not wholly in view.

import { createContext, destroyContext, domToPng } from "modern-screenshot";
import { backgroundBehind, outlineColors } from "./target";
import { type TextPoint, type TextWindow, charsBetween, nextText, pointAt, pointInto, prevText, windowAround } from "./text-walk";

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

/** How `renderClip` renders: `crop`, the part of the element to render
 * (relative to its top left corner; all of it when absent); `style`, more
 * inline style for the rendered root. */
export interface ClipOptions {
  crop?: { x: number; y: number; w: number; h: number };
  style?: Partial<CSSStyleDeclaration>;
}

/** A PNG of `el` (or of its `crop`); rejects when it has no size or rendering exceeds `timeoutMs`. */
export async function renderClip(el: Element, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS, opts: ClipOptions = {}): Promise<ArrayBuffer> {
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) throw new Error("the anchored element has no size");
  const origin = new URL(win.location.href).origin;
  const crop = opts.crop;
  // A crop renders the element at its own size, moved so the crop sits in a
  // picture of the crop's size (the declarations replace the forced size).
  const cropStyle: Partial<CSSStyleDeclaration> = crop
    ? { width: `${r.width}px`, height: `${r.height}px`, transform: `translate(${-crop.x}px, ${-crop.y}px)`, transformOrigin: "0 0" }
    : {};
  const png = (async () => {
    const ctx = await createContext(el, {
      scale: clipScale(crop?.w ?? r.width, crop?.h ?? r.height, win.devicePixelRatio || 1),
      filter: n => !crossOriginImage(n, origin),
      fetch: { placeholderImage: TRANSPARENT },
      backgroundColor: clipBackground(el, win),
      style: { ...clipRootStyle(el, win), ...opts.style, ...cropStyle },
      timeout: timeoutMs,
      ...(crop ? { width: crop.w, height: crop.h } : {}),
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

/** The span along one axis of an element from `start` of length `size`, in
 * view from `visStart` to `visEnd`: all of it when it is at most `max` or
 * wholly in view; else its part in view (or its edge nearest the view), grown
 * equally both ways within the element to `max`, and cut at the far end when
 * the part in view alone exceeds `max`. Returns the offset from `start` and
 * the length. */
function cropSpan(start: number, size: number, visStart: number, visEnd: number, max: number): [number, number] {
  const end = start + size;
  if (size <= max || (start >= visStart && end <= visEnd)) return [0, Math.round(size)];
  let a = Math.min(end, Math.max(start, visStart));
  let b = Math.max(a, Math.min(end, Math.max(start, visEnd)));
  const need = Math.min(size, max);
  if (b - a > need) b = a + need;
  let grow = need - (b - a);
  const up = Math.min(grow / 2, a - start);
  a -= up; grow -= up;
  const down = Math.min(grow, end - b);
  b += down; grow -= down;
  a -= Math.min(grow, a - start);
  return [Math.round(a - start), Math.round(b - a)];
}

/** The part of an element at `r` (client coordinates) a clip of it renders,
 * relative to its top left corner. Each axis is whole when it is within the
 * budget or wholly in the viewport `vp` (the picture is then scaled down);
 * otherwise it is the element's part in view, grown within the element to the
 * budget. */
export function elementRegion(r: { left: number; top: number; width: number; height: number }, vp: { w: number; h: number }): { x: number; y: number; w: number; h: number } {
  const [x, w] = cropSpan(r.left, r.width, 0, vp.w, MAX_CLIP_REGION.w);
  const [y, h] = cropSpan(r.top, r.height, 0, vp.h, MAX_CLIP_REGION.h);
  return { x, y, w, h };
}

/** A PNG of `t` within the clip budget: an element that fits it, or a range's
 * nearest block ancestor when that fits, is rendered whole; a larger element
 * is rendered cropped to `elementRegion` (whole, scaled down, when that crops
 * nothing); a range in a larger block is
 * rendered as the region around it (`renderRegion`). */
export async function renderTargetClip(t: Element | Range, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  if ("nodeType" in t) {
    const r = t.getBoundingClientRect();
    if (fitsClipBudget(r)) return renderClip(t, win, timeoutMs);
    const doc = t.ownerDocument;
    const vp = { w: doc.documentElement.clientWidth || win.innerWidth, h: doc.documentElement.clientHeight || win.innerHeight };
    const crop = elementRegion(r, vp);
    const whole = crop.x === 0 && crop.y === 0 && crop.w === Math.round(r.width) && crop.h === Math.round(r.height);
    return renderClip(t, win, timeoutMs, whole ? {} : { crop });
  }
  const range = t;
  const block = blockAncestor(range.commonAncestorContainer, win);
  if (fitsClipBudget(block.getBoundingClientRect())) return renderClip(block, win, timeoutMs);
  return renderRegion(block, range, win, timeoutMs);
}

/** How long an area clip may take: longer than other clips, since the area's
 * element (often the page's main column) is rendered to crop it, and an
 * area thread's clip is the record of what the viewer drew around. */
export const AREA_CLIP_TIMEOUT_MS = 12_000;

/** The part of `box` (an element's border box) that `r` covers, relative to
 * the box's top left corner, rounded to whole CSS px and at least 1 px each way. */
export function areaCrop(r: { x: number; y: number; w: number; h: number }, box: { left: number; top: number }): { x: number; y: number; w: number; h: number } {
  return { x: Math.round(r.x - box.left), y: Math.round(r.y - box.top), w: Math.max(1, Math.round(r.w)), h: Math.max(1, Math.round(r.h)) };
}

/** A PNG of exactly the drawn area `r` (viewport pixels, within `el`'s box)
 * as the page shows it now: `el` rendered and cropped to `r`. */
export async function renderAreaClip(el: Element, r: { x: number; y: number; w: number; h: number }, win: Window = window, timeoutMs = AREA_CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  return renderClip(el, win, timeoutMs, { crop: areaCrop(r, el.getBoundingClientRect()) });
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

/** The first text point at or after `from` in `root` whose character ends
 * below `y` (client coordinates), measuring whole text nodes until one reaches
 * below it; null when none does. */
function pointBelow(root: Node, from: TextPoint, y: number): TextPoint | null {
  const r = root.ownerDocument!.createRange();
  let off = from.offset;
  for (let t: Text | null = from.node, n = 0; t && n < 100_000; t = nextText(root, t), n++, off = 0) {
    if (off >= t.length) continue;
    r.setStart(t, off);
    r.setEnd(t, t.length);
    const b = typeof r.getBoundingClientRect === "function" ? r.getBoundingClientRect() : null;
    if (!b || (!b.width && !b.height) || b.bottom <= y) continue;
    const w: TextWindow = { segs: [{ node: t, from: 0, to: t.length }], text: t.data, at: off };
    let lo = off;
    let hi = t.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      const c = charRect(w, mid, 1);
      if (!c || c.bottom > y) hi = mid; else lo = mid + 1;
    }
    return { node: t, offset: lo };
  }
  return null;
}

/** Media and embedded content, replaced by placeholders in a region copy. */
const EMBEDS = "iframe, video, audio, object, embed";
/** Elements whose `name` or `form` would tie a copy to the page's forms. */
const FORM_PARTS = "input, select, textarea, button, fieldset, output, object, form";

/** Whether `el` is an autonomous custom element its window has defined. */
function definedCustom(el: Element): boolean {
  return el.localName.includes("-") && !!el.ownerDocument.defaultView?.customElements?.get(el.localName);
}

/** Whether `el` becomes a placeholder in a region copy. */
export const replacedInCopy = (el: Element) => el.matches(EMBEDS) || definedCustom(el);

/** A `div` (or `span`, for an inline `el`) with `el`'s attributes but `is`:
 * a copy of `el` that runs no custom element code. */
export function plainCopy(el: Element): HTMLElement {
  const doc = el.ownerDocument;
  const inline = (doc.defaultView?.getComputedStyle(el).display || "inline").startsWith("inline");
  const out = doc.createElement(inline ? "span" : "div");
  for (const a of Array.from(el.attributes)) if (a.name !== "is") out.setAttribute(a.name, a.value);
  return out;
}

/** Makes a region copy inert before it is connected, so connecting it changes
 * nothing on the page: media, embedded content, and defined custom elements
 * become empty placeholders of the size of `originals` (the page's elements
 * they were copied from, in document order; their width and height attributes
 * when those do not line up); form controls and forms lose `name` and `form`
 * (a copied checked radio would otherwise uncheck the reader's); nothing keeps
 * `autofocus`; and the copy is `inert` and `aria-hidden`. */
export function neutraliseCopy(copy: HTMLElement, originals: Element[]): void {
  const doc = copy.ownerDocument;
  const replaced = Array.from(copy.querySelectorAll("*")).filter(replacedInCopy);
  const sized = originals.length === replaced.length;
  replaced.forEach((el, i) => {
    if (!copy.contains(el)) return; // inside one already replaced
    const b = sized ? originals[i].getBoundingClientRect() : null;
    const w = b ? b.width : Number(el.getAttribute("width")) || 0;
    const h = b ? b.height : Number(el.getAttribute("height")) || 0;
    const ph = doc.createElement("span");
    for (const [k, v] of Object.entries({ display: "inline-block", width: `${w}px`, height: `${h}px`, "vertical-align": "bottom" })) ph.style.setProperty(k, v, "important");
    el.replaceWith(ph);
  });
  for (const el of [copy, ...Array.from(copy.querySelectorAll("*"))]) {
    if (el.matches(FORM_PARTS)) { el.removeAttribute("name"); el.removeAttribute("form"); }
    el.removeAttribute("autofocus");
  }
  copy.setAttribute("inert", "");
  copy.setAttribute("aria-hidden", "true");
}

/** Bands of `tint` over the text of `copy` from `from` characters in to
 * `from + length` (one band per line), appended to it so they render with it. */
function markPicked(copy: HTMLElement, from: number, length: number, tint: string): void {
  const s = pointInto(copy, from);
  if (!s) return;
  // A pick longer than the region is marked to the region's end.
  const e = pointInto(copy, from + length);
  const r = copy.ownerDocument.createRange();
  r.setStart(s.node, s.offset);
  if (e) r.setEnd(e.node, e.offset); else r.setEnd(copy, copy.childNodes.length);
  const lines: { left: number; top: number; right: number; bottom: number }[] = [];
  const rects = Array.from(r.getClientRects?.() ?? []).filter(b => b.width > 0 && b.height > 0).sort((x, y) => x.top - y.top);
  for (const b of rects) {
    const mid = (b.top + b.bottom) / 2;
    const line = lines.find(l => mid >= l.top && mid <= l.bottom);
    if (line) Object.assign(line, { left: Math.min(line.left, b.left), right: Math.max(line.right, b.right), top: Math.min(line.top, b.top), bottom: Math.max(line.bottom, b.bottom) });
    else lines.push({ left: b.left, top: b.top, right: b.right, bottom: b.bottom });
  }
  const box = copy.getBoundingClientRect();
  for (const l of lines) {
    const band = copy.ownerDocument.createElement("div");
    const style: Record<string, string> = {
      position: "absolute", display: "block", margin: "0", padding: "0", border: "0", "border-radius": "2px", "pointer-events": "none",
      // The rendered copy is not positioned (modern-screenshot drops its
      // position) and sits at the picture's origin, so the bands are placed
      // from its border box.
      left: `${l.left - box.left}px`, top: `${l.top - box.top}px`,
      width: `${l.right - l.left}px`, height: `${l.bottom - l.top}px`, background: tint,
    };
    for (const [k, v] of Object.entries(style)) band.style.setProperty(k, v, "important");
    copy.appendChild(band);
  }
}

/** A PNG of the region around `range` in `block` (see `regionBounds`), at the
 * block's width, with the picked text marked by bands of the outline's tint. The lines in the region are copied into a shallow copy of
 * the block (with shallow copies of the elements between them, so the page's
 * styles still apply), placed next to it off-screen, rendered, and removed. */
export async function renderRegion(block: Element, range: Range, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  const doc = block.ownerDocument;
  const br = block.getBoundingClientRect();
  const tr = range.getBoundingClientRect();
  const { top, bottom } = regionBounds(tr, br);
  const s = textAfter(block, range.startContainer, range.startOffset);
  const e = textBefore(block, range.endContainer, range.endOffset);
  // A range taller than the budget: the region ends near its own bottom, not
  // at the range's end, so the rest of the range is never copied.
  const cut = bottom < Math.min(br.bottom, tr.bottom + REGION_PAD);
  const part = doc.createRange();
  // Where the copy's text starts, to find the picked text in it.
  let origin: TextPoint | null = s;
  if (s && e) {
    const above = windowAround(block, s.node, s.offset, NEVER);
    const endNear = (cut && pointBelow(block, s, bottom)) || e;
    const below = windowAround(block, endNear.node, endNear.offset, NEVER);
    const a = pointAt(above, regionStart(above, top), false);
    const b = pointAt(below, regionEnd(below, bottom), true);
    part.setStart(a.node, a.offset);
    part.setEnd(b.node, b.offset);
    origin = a;
    if (part.collapsed) { part.setStart(range.startContainer, range.startOffset); part.setEnd(range.endContainer, range.endOffset); origin = s; }
  } else {
    part.setStart(range.startContainer, range.startOffset);
    part.setEnd(range.endContainer, range.endOffset);
  }
  // Copying runs the constructors of defined custom elements inside the part
  // (cloning creates them); the copy then replaces them before it is
  // connected, so their connected callbacks never run.
  let inner: Node = part.cloneContents();
  const common = part.commonAncestorContainer;
  const commonEl = common.nodeType === Node.ELEMENT_NODE ? (common as Element) : common.parentElement;
  for (let a = commonEl; a && a !== block && block.contains(a); a = a.parentElement) {
    const wrap = definedCustom(a) ? plainCopy(a) : a.cloneNode(false);
    wrap.appendChild(inner);
    inner = wrap;
  }
  const copy = definedCustom(block) ? plainCopy(block) : (block.cloneNode(false) as HTMLElement);
  copy.appendChild(inner);
  // The page's elements behind the copy's placeholders, looked up only when there are any.
  const originals = copy.querySelector("*") && Array.from(copy.querySelectorAll("*")).some(replacedInCopy) && commonEl
    ? Array.from(commonEl.querySelectorAll("*")).filter(el => replacedInCopy(el) && part.intersectsNode(el))
    : [];
  neutraliseCopy(copy, originals);
  const fixed: Record<string, string> = {
    position: "fixed", left: "-100000px", top: "0", width: `${br.width}px`, "box-sizing": "border-box",
    height: "auto", "min-height": "0", "max-height": "none", margin: "0", overflow: "hidden",
    transform: "none", "pointer-events": "none",
  };
  for (const [k, v] of Object.entries(fixed)) copy.style?.setProperty(k, v, "important");
  block.after(copy);
  try {
    if (s && e && origin) markPicked(copy, charsBetween(block, origin, s), charsBetween(block, s, e), outlineColors(backgroundBehind(block)).tint);
    return await renderClip(copy, win, timeoutMs);
  } finally {
    copy.remove();
  }
}
