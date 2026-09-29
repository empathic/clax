// Screenshot clips of the anchored region (spec §9 "Clips"), rendered in the
// frame with modern-screenshot. Images from other origins are left out (they
// would taint the canvas); in an opaque-origin sandbox every fetched image is
// cross-origin to the page, so those clips carry placeholders for images.
// modern-screenshot reads browser default styles from a helper `srcdoc` iframe;
// in an opaque-origin sandbox that iframe inherits the sandbox and cannot be
// read, so there it gets a never-attached iframe instead and every computed
// style is inlined.

import { createContext, destroyContext, domToPng } from "modern-screenshot";

export const MAX_SIDE = 1600;
export const CLIP_TIMEOUT_MS = 4000;
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
