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
