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
//
// Every render is private (`privateRender`): what is typed into fields and
// editable regions is blank in it unless the field (or what holds it) carries
// `data-clax-show`, and a password never shows; an element marked
// `data-clax-redact` is drawn as a solid block of its box; what carries
// `data-clax-ignore`, and Clax's own overlay, is left out. Each render also
// lists what it could not draw (`Omitted`), so a reporter can be offered the
// browser's own screenshot instead.

import { type Options, createContext, destroyContext, domToPng } from "modern-screenshot";
import { REDACT_ATTR, SHOW_ATTR, UNRENDERED } from "./anchor";
import { blockAncestor } from "./block";
import { backgroundBehind, outlineColors } from "./target";
import { type TextPoint, type TextWindow, charsBetween, nextText, pointAt, pointInto, prevText, windowAround } from "./text-walk";

export { blockAncestor } from "./block";

export const MAX_SIDE = 1600;
export const CLIP_TIMEOUT_MS = 4000;
/** The largest region, in CSS px, a clip renders. */
export const MAX_CLIP_REGION = { w: 1600, h: 2400 };
/** How far above and below a range its region clip reaches, in CSS px. */
export const REGION_PAD = 120;

type Extent = { top: number; bottom: number; width: number; height: number };

/** A rectangle in CSS px. */
export type Rect = { x: number; y: number; w: number; h: number };

/** Something a DOM render could not draw (a cross-origin iframe, object or
 * embed; an image from another origin; a canvas it may not read; a video
 * whose frame it cannot load), and where it is in the clip: `rect` is in the
 * clip image's own pixels, from its top left corner (not the viewport's CSS
 * px), cut to the image, so it outlines the omission exactly on the image. */
export type Omitted = { kind: "iframe" | "image" | "canvas" | "video"; rect: Rect };

/** A DOM render: its PNG, and what it left out (`omitted`, in the image's
 * pixels). `at` is the part of the viewport the image shows (viewport CSS
 * px; for a region clip, the region on the page) and `scale` the image's
 * pixels per CSS px, so a viewport rectangle maps into the image as
 * `(x - at.x) * scale`. */
export interface RenderResult {
  png: ArrayBuffer;
  omitted: Omitted[];
  at: Rect;
  scale: number;
}

/** `r` (viewport CSS px) in the pixels of an image of `at` drawn at `scale`. */
export function toImage(r: Rect, at: Rect, scale: number): Rect {
  const x = Math.round((r.x - at.x) * scale);
  const y = Math.round((r.y - at.y) * scale);
  return { x, y, w: Math.max(1, Math.round((r.x + r.w - at.x) * scale) - x), h: Math.max(1, Math.round((r.y + r.h - at.y) * scale) - y) };
}

/** The most omissions a render lists, the largest first, so that a report's
 * context stays within its bound. */
export const MAX_OMITTED = 32;

/** Whether a region of `r`'s size fits the clip budget. */
export function fitsClipBudget(r: { width: number; height: number }): boolean {
  return r.width <= MAX_CLIP_REGION.w && r.height <= MAX_CLIP_REGION.h;
}

/** The vertical extent of the region clip for a range at `target` inside
 * `block` (client coordinates): `REGION_PAD` above and below the range, within
 * the block, at most `MAX_CLIP_REGION.h` tall (cut at the bottom). */
export function regionBounds(target: Extent, block: Extent): { top: number; bottom: number } {
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
 * inline style for the rendered root; `omitted`, what the render leaves out
 * (in viewport CSS px) and the part of the viewport the picture stands for
 * (`at`), when the caller knows them better than the rendered element does
 * (a region's copy stands off-screen for the page's elements); `viewport`,
 * that the root is rendered as the viewport shows it, so fixed and stuck
 * elements are drawn where they show now. */
export interface ClipOptions {
  crop?: { x: number; y: number; w: number; h: number };
  style?: Partial<CSSStyleDeclaration>;
  omitted?: { at: Rect; list: Omitted[] };
  viewport?: boolean;
}

/** Documents whose policy refused the <style> modern-screenshot reads web
 * fonts' @import rules into (a CSP `style-src` without 'unsafe-inline'): their
 * clips are rendered without web fonts, in the fonts the browser falls back to. */
const fontsRefused = new WeakSet<Document>();

/** Renders `doc`'s clips without web fonts from the first: for a page whose
 * policy refuses inline <style>, so no clip trips it (modern-screenshot reads
 * fonts' @import rules through an inline <style>; refused, it fails and the
 * clip is drawn again without them). */
export function withoutWebFonts(doc: Document): void {
  fontsRefused.add(doc);
}

/** Whether `doc`'s own `<meta http-equiv="Content-Security-Policy">` refuses
 * inline <style> (its `style-src-elem`, else `style-src`, else `default-src`
 * without 'unsafe-inline', or with a nonce or hash). A policy sent as a header cannot be read from
 * the page: its host says so (`withoutWebFonts`). */
export function metaRefusesInlineStyle(doc: Document): boolean {
  for (const m of Array.from(doc.querySelectorAll<HTMLMetaElement>('meta[http-equiv="Content-Security-Policy" i]'))) {
    const directives = new Map(m.content.split(";").map(d => d.trim().split(/\s+/)).filter(d => d[0]).map(([k, ...v]) => [k.toLowerCase(), v]));
    const style = directives.get("style-src-elem") ?? directives.get("style-src") ?? directives.get("default-src");
    // A nonce or a hash makes a browser ignore 'unsafe-inline'.
    if (style && (!style.includes("'unsafe-inline'") || style.some(v => /^'(nonce|sha256|sha384|sha512)-/.test(v)))) return true;
  }
  return false;
}

/** Whether `e` is modern-screenshot failing on that refused <style>, whose
 * `sheet` is null (each engine's message names the `cssRules` it read). */
function fontSheetRefused(e: unknown): boolean {
  return e instanceof TypeError && e.message.includes("cssRules");
}

/** The largest clip the daemon keeps, in bytes (its `MAX_CLIP_BYTES`). */
export const MAX_CLIP_BYTES = 5 * 1024 * 1024;
/** How many times a clip over `MAX_CLIP_BYTES` is rendered again at half the scale. */
export const CLIP_SHRINKS = 3;

/** A PNG of `el` (or of its `crop`) of at most `MAX_CLIP_BYTES`: one over
 * that is rendered again at half the scale, up to `CLIP_SHRINKS` times.
 * Rejects when the element has no size, when the PNG is still too large, or
 * when rendering exceeds `timeoutMs` in all. */
export async function renderClip(el: Element, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS, opts: ClipOptions = {}): Promise<ArrayBuffer> {
  return (await renderClipWithReport(el, win, timeoutMs, opts)).png;
}

/** `renderClip`'s PNG, private (`privateRender`), with what it left out. */
export async function renderClipWithReport(el: Element, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS, opts: ClipOptions = {}): Promise<RenderResult> {
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) throw new Error("the anchored element has no size");
  const origin = new URL(win.location.href).origin;
  const crop = opts.crop;
  // The picture's place in the viewport.
  const at: Rect = crop ? { x: r.left + crop.x, y: r.top + crop.y, w: crop.w, h: crop.h } : { x: r.left, y: r.top, w: r.width, h: r.height };
  const seen = survey(el, origin, at, !!opts.viewport);
  const shown = opts.omitted?.at ?? at;
  const omitted = opts.omitted?.list ?? seen.omitted;
  let drawnAt = 1;
  // A crop renders the element at its own size, moved so the crop sits in a
  // picture of the crop's size (the declarations replace the forced size).
  const cropStyle: Partial<CSSStyleDeclaration> = crop
    ? { width: `${r.width}px`, height: `${r.height}px`, transform: `translate(${-crop.x}px, ${-crop.y}px)`, transformOrigin: "0 0" }
    : {};
  const doc = el.ownerDocument;
  const background = clipBackground(el, win);
  const once = async (scale: number) => {
    const ctx = await createContext(el, {
      ...(fontsRefused.has(doc) ? { font: false as const } : {}),
      scale,
      ...privateRender(el, origin, seen, blockColour(background)),
      fetch: { placeholderImage: TRANSPARENT },
      backgroundColor: background,
      style: { ...clipRootStyle(el, win), ...opts.style, ...cropStyle },
      timeout: timeoutMs,
      ...(crop ? { width: crop.w, height: crop.h } : {}),
    });
    if (win.origin === "null") ctx.sandbox = el.ownerDocument.createElement("iframe");
    try {
      return dataUrlToBuffer(await domToPng(ctx));
    } finally {
      destroyContext(ctx);
    }
  };
  const png = (async () => {
    let scale = clipScale(crop?.w ?? r.width, crop?.h ?? r.height, win.devicePixelRatio || 1);
    for (let shrinks = 0; ; shrinks++) {
      let buf: ArrayBuffer;
      try {
        buf = await once(scale);
      } catch (e) {
        if (fontsRefused.has(doc) || !fontSheetRefused(e)) throw e;
        fontsRefused.add(doc);
        buf = await once(scale);
      }
      if (buf.byteLength <= MAX_CLIP_BYTES) { drawnAt = scale; return buf; }
      if (shrinks >= CLIP_SHRINKS) throw new Error("the screenshot is too large to keep, even scaled down");
      scale /= 2;
    }
  })();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`clip took longer than ${timeoutMs} ms`)), timeoutMs); });
  try {
    const buf = await Promise.race([png, late]);
    return { png: buf, omitted: omitted.map(o => ({ kind: o.kind, rect: toImage(o.rect, shown, drawnAt) })), at: shown, scale: drawnAt };
  } finally {
    clearTimeout(timer);
  }
}

/** The part of `a` inside `b`, rounded to whole CSS px; null when they do not overlap. */
export function intersect(a: Rect, b: Rect): Rect | null {
  const x = Math.max(a.x, b.x);
  const y = Math.max(a.y, b.y);
  const right = Math.min(a.x + a.w, b.x + b.w);
  const bottom = Math.min(a.y + a.h, b.y + b.h);
  if (right <= x || bottom <= y) return null;
  const rx = Math.round(x);
  const ry = Math.round(y);
  return { x: rx, y: ry, w: Math.max(1, Math.round(right) - rx), h: Math.max(1, Math.round(bottom) - ry) };
}

const rectOf = (el: Element): Rect => {
  const b = el.getBoundingClientRect();
  return { x: b.left, y: b.top, w: b.width, h: b.height };
};

/** `found` cut to the `MAX_OMITTED` largest. */
function largest(found: Omitted[]): Omitted[] {
  return found.sort((a, b) => b.rect.w * b.rect.h - a.rect.w * a.rect.h).slice(0, MAX_OMITTED);
}

/** The document an iframe shows, when this page may read it and it is of
 * this page's origin; null otherwise (another origin's, a sandbox's without
 * `allow-same-origin`, or one not loaded). */
function readableFrame(f: Element, origin: string): Document | null {
  try {
    const src = f.getAttribute("src");
    if (src && !f.hasAttribute("srcdoc")) {
      const u = new URL(src, f.baseURI);
      if ((u.protocol === "http:" || u.protocol === "https:") && u.origin !== origin) return null;
    }
    const d = (f as HTMLIFrameElement).contentDocument;
    return d?.documentElement ? d : null;
  } catch {
    return null;
  }
}

/** Whether a render can draw `v`'s current frame: it loads the video again
 * from its URL, which works for this origin's files and data URLs only. */
function drawableVideo(v: Element, origin: string): boolean {
  const src = (v as HTMLVideoElement).currentSrc;
  if (!src) return false;
  try {
    const u = new URL(src);
    return u.protocol === "data:" || ((u.protocol === "http:" || u.protocol === "https:") && u.origin === origin);
  } catch {
    return false;
  }
}

/** Whether this page may read `c`'s pixels (a canvas that drew another
 * origin's image may not). */
function readableCanvas(c: Element): boolean {
  try {
    (c as HTMLCanvasElement).toDataURL();
    return true;
  } catch {
    return false;
  }
}

/** What a render of `el` cannot draw, if anything. */
function omission(el: Element, origin: string): Omitted["kind"] | null {
  switch (el.localName) {
    case "iframe": return readableFrame(el, origin) ? null : "iframe";
    case "object": case "embed": return "iframe";
    case "canvas": return readableCanvas(el) ? null : "canvas";
    case "video": return drawableVideo(el, origin) ? null : "video";
    default: return null;
  }
}

/** Whether a render of `root` leaves `n` (one of its descendants) out: an
 * image from another origin (it would taint the canvas), Clax's overlay, or
 * what carries `data-clax-ignore`; and the hidden frame modern-screenshot
 * attaches to the page to read default styles from, while it renders. */
function leftOut(n: Node, origin: string): boolean {
  if (n.nodeType !== Node.ELEMENT_NODE) return false;
  const el = n as Element;
  return crossOriginImage(el, origin) || el.matches(UNRENDERED) || (el.localName === "iframe" && el.id.startsWith("__SANDBOX__"));
}

/** Elements whose drawn copy keeps nothing of them (modern-screenshot draws
 * a canvas or a video as a new image, a readable iframe as its document) or
 * whose content paints over their background: a redaction of one is drawn
 * over the picture instead. */
const REPLACED = new Set(["img", "canvas", "video", "iframe", "object", "embed"]);

/** What a render needs to know of the page before it clones it. */
interface Survey {
  omitted: Omitted[];
  /** Redacted replaced elements' boxes, relative to the picture's top left corner. */
  blocks: Rect[];
  /** Whether the rendered root is in a `data-clax-show` region. */
  shown: boolean;
  /** Whether the rendered root is in an editable region. */
  editable: boolean;
  /** For a viewport render, where each fixed or stuck element (in the
   * order the renderer meets them) is drawn, relative to the root's box. */
  pinned: { fixed: boolean; dx: number; dy: number }[] | null;
}

/** The nearest element at or above `el` that matches `sel`, across shadow roots. */
function closestComposed(el: Element, sel: string): Element | null {
  for (let cur: Element | null = el; cur; ) {
    const hit = cur.closest(sel);
    if (hit) return hit;
    const host: Element | undefined = (cur.getRootNode() as ShadowRoot).host;
    cur = host ?? null;
  }
  return null;
}

/** Whether a fixed element's containing block is the viewport: no ancestor
 * makes one for it (a transform, a filter, a perspective, paint containment). */
function viewportFixed(el: Element, win: Window): boolean {
  for (let a = el.parentElement; a; a = a.parentElement) {
    const cs = win.getComputedStyle(a);
    if ((cs.transform && cs.transform !== "none") || (cs.filter && cs.filter !== "none") || (cs.perspective && cs.perspective !== "none")
      || /paint|layout|strict|content/.test(cs.contain ?? "") || /transform|filter|perspective/.test(cs.willChange ?? "")) return false;
  }
  return true;
}

/** Where a stuck element shows, against where it would be were it not stuck:
 * the element is unstuck for one synchronous layout and put back. */
function stuckOffset(el: HTMLElement): { dx: number; dy: number } {
  const stuck = el.getBoundingClientRect();
  const prior = el.style.getPropertyValue("position");
  const priority = el.style.getPropertyPriority("position");
  el.style.setProperty("position", "static", "important");
  const free = el.getBoundingClientRect();
  if (prior) el.style.setProperty("position", prior, priority); else el.style.removeProperty("position");
  return { dx: stuck.left - free.left, dy: stuck.top - free.top };
}

/** Walks what a render of `root` draws, in the order modern-screenshot clones
 * it (through shadow roots, slots and readable iframes; never into what
 * `leftOut` drops, a video, or a style or script): what it cannot draw within
 * `at`, the redacted replaced elements, and for a `viewport` render the
 * fixed and stuck elements. */
function survey(root: Element, origin: string, at: Rect, viewport: boolean): Survey {
  const found: Omitted[] = [];
  const blocks: Rect[] = [];
  const pinned: Survey["pinned"] = viewport ? [] : null;
  const rootBox = root.getBoundingClientRect();
  const stack: { el: Element; redacted: boolean }[] = [{ el: root, redacted: false }];
  while (stack.length) {
    const { el, redacted: under } = stack.pop()!;
    const redacted = under || el.hasAttribute(REDACT_ATTR);
    const win = el.ownerDocument.defaultView;
    if (!redacted) {
      const kind = omission(el, origin);
      const cut = kind && intersect(rectOf(el), at);
      if (kind && cut) found.push({ kind, rect: cut });
    } else if (!under && REPLACED.has(el.localName)) {
      const b = rectOf(el);
      blocks.push({ x: b.x - at.x, y: b.y - at.y, w: b.w, h: b.h });
    }
    if (pinned && win && el !== root) {
      const pos = win.getComputedStyle(el).position;
      if (pos === "fixed") {
        const b = el.getBoundingClientRect();
        pinned.push(viewportFixed(el, win) ? { fixed: true, dx: b.left - rootBox.left, dy: b.top - rootBox.top } : { fixed: true, dx: NaN, dy: NaN });
      } else if (pos === "sticky" && el instanceof win.HTMLElement) {
        pinned.push({ fixed: false, ...stuckOffset(el) });
      }
    }
    if (el.localName === "video") continue;
    let kids: Element[] = [];
    if (el.localName === "iframe") {
      const d = readableFrame(el, origin);
      if (d) kids = [d.documentElement];
    } else {
      for (const c of Array.from((el.shadowRoot ?? el).children)) {
        if (c.localName === "slot" && typeof (c as HTMLSlotElement).assignedElements === "function") kids.push(...(c as HTMLSlotElement).assignedElements());
        else kids.push(c);
      }
      kids = kids.filter(c => c.localName !== "style" && c.localName !== "script");
    }
    for (let i = kids.length - 1; i >= 0; i--) {
      const c = kids[i];
      if (leftOut(c, origin)) {
        if (!redacted && c.localName === "img") {
          const cut = intersect(rectOf(c), at);
          if (cut) found.push({ kind: "image", rect: cut });
        }
        continue;
      }
      stack.push({ el: c, redacted });
    }
  }
  const editableAt = closestComposed(root, "[contenteditable]");
  return {
    omitted: largest(found),
    blocks,
    shown: !!closestComposed(root, `[${SHOW_ATTR}]`),
    editable: !!editableAt && editableAt.getAttribute("contenteditable")?.toLowerCase() !== "false",
    pinned,
  };
}

/** The colour a redaction is drawn in: black on a light page, white on a dark one. */
export function blockColour(background: string): "#000" | "#fff" {
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(background);
  const rgb = hex ? hex.slice(1).map(h => parseInt(h, 16)) : (/rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/.exec(background)?.slice(1).map(Number) ?? [255, 255, 255]);
  const [r, g, b] = rgb;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 128 ? "#fff" : "#000";
}

/** Marks the first option of the copied select `el` whose value is `value`
 * as selected, and no other. */
function selectValue(el: Element, value: string | null): void {
  if (value === null) return;
  let found = false;
  for (const o of Array.from(el.querySelectorAll("option"))) {
    // An option without a value attribute has its text, whitespace collapsed.
    const v = o.getAttribute("value") ?? (o.textContent ?? "").replace(/\s+/g, " ").trim();
    if (!found && v === value) { o.setAttribute("selected", ""); found = true; } else o.removeAttribute("selected");
  }
}

/** Input types whose value is a label or nothing shown, not something typed. */
const LABELLED = new Set(["button", "submit", "reset", "image", "checkbox", "radio"]);

/** Blanks what is typed in the copy `root` (modern-screenshot's clone of the
 * page): fields' values, selections, and the text of editable regions,
 * except within `data-clax-show` (`shown`: the root is); passwords always. */
function blankTyped(root: Element, shown: boolean, editable: boolean): void {
  const isShown = (el: Element) => shown || !!el.closest(`[${SHOW_ATTR}]`);
  for (const el of [root, ...Array.from(root.querySelectorAll("input, textarea, select"))]) {
    const tag = el.localName;
    if (tag === "input") {
      const type = (el.getAttribute("type") ?? "").toLowerCase();
      if (type !== "password" && (LABELLED.has(type) || isShown(el))) continue;
      el.removeAttribute("value");
      (el as HTMLInputElement).value = "";
    } else if (tag === "textarea") {
      // A textarea draws its text, not the value attribute modern-screenshot
      // copies the typed value into: a shown one gets that value as its text.
      if (isShown(el)) { const v = el.getAttribute("value"); if (v !== null) el.textContent = v; continue; }
      el.removeAttribute("value");
      el.textContent = "";
    } else if (tag === "select") {
      // A select draws its selected option: a shown one selects the option
      // of the value modern-screenshot copied (the first of a multiple
      // select's choices), and only that one.
      if (isShown(el)) { selectValue(el, el.getAttribute("value")); continue; }
      el.removeAttribute("value");
      for (const o of Array.from(el.querySelectorAll("option, optgroup"))) {
        o.removeAttribute("selected");
        o.removeAttribute("label");
        if (o.localName === "option") o.textContent = "";
      }
    }
  }
  if (!editable && !root.querySelector("[contenteditable]")) return;
  const walk = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let t = walk.nextNode() as Text | null; t; t = walk.nextNode() as Text | null) {
    const p = t.parentElement;
    if (!p) continue;
    const holder = p.closest("[contenteditable]");
    const typed = holder ? holder.getAttribute("contenteditable")?.toLowerCase() !== "false" : editable;
    if (typed && !isShown(p)) t.data = "";
  }
}

/** Draws each fixed and stuck element of a viewport copy where the page
 * shows it now (`pinned`, in the order of the copy's elements). The copy's
 * root is the containing block of its fixed elements, so each is placed from
 * the root's top left corner; a stuck one is moved by how far it is stuck.
 * Nothing moves when the copy's fixed and stuck elements do not line up
 * with the page's. */
function pinElements(root: Element, pinned: NonNullable<Survey["pinned"]>): void {
  const copies = Array.from(root.querySelectorAll<HTMLElement>("*")).filter(e => e.style && (e.style.position === "fixed" || e.style.position === "sticky"));
  if (copies.length !== pinned.length) return;
  copies.forEach((c, i) => {
    const p = pinned[i];
    if (p.fixed !== (c.style.position === "fixed") || Number.isNaN(p.dx)) return;
    const set = (k: string, v: string) => c.style.setProperty(k, v, "important");
    if (p.fixed) { set("left", `${p.dx}px`); set("top", `${p.dy}px`); set("margin", "0"); }
    else { set("position", "relative"); set("left", `${p.dx}px`); set("top", `${p.dy}px`); }
    set("right", "auto");
    set("bottom", "auto");
  });
}

/** The modern-screenshot options that make a render of `root` private:
 * images from other origins, Clax's overlay and what carries
 * `data-clax-ignore` are left out; typed values are blanked (`blankTyped`);
 * every `data-clax-redact` element is drawn as a block of `colour` (a rule
 * in the picture's style that paints its box and hides what it holds, and,
 * for a replaced element, a block drawn over the picture at its box). */
function privateRender(root: Element, origin: string, seen: Survey, colour: string): Pick<Options, "filter" | "onCloneNode" | "onCreateForeignObjectSvg"> {
  return {
    filter: n => !leftOut(n, origin),
    onCloneNode: clone => {
      if (clone.nodeType !== Node.ELEMENT_NODE) return;
      blankTyped(clone as Element, seen.shown, seen.editable);
      if (seen.pinned) pinElements(clone as Element, seen.pinned);
    },
    onCreateForeignObjectSvg: svg => {
      const doc = svg.ownerDocument;
      const style = doc.createElement("style");
      const a = `[${REDACT_ATTR}]`;
      style.textContent = `${a}{background:${colour}!important;border-color:${colour}!important;color:transparent!important;-webkit-text-fill-color:transparent!important;`
        + "text-shadow:none!important;box-shadow:none!important;outline:none!important;text-decoration-color:transparent!important;caret-color:transparent!important}"
        + `${a} *,${a}::before,${a}::after,${a} ::before,${a} ::after,${a}::marker,${a} ::marker{visibility:hidden!important}`;
      svg.appendChild(style);
      for (const b of seen.blocks) {
        const rect = doc.createElementNS("http://www.w3.org/2000/svg", "rect");
        for (const [k, v] of Object.entries({ x: b.x, y: b.y, width: b.w, height: b.h, fill: colour })) rect.setAttribute(k, String(v));
        rect.setAttribute("data-clax-redacted", "");
        svg.appendChild(rect);
      }
    },
  };
}

/** A PNG of what the viewport of `win` shows now: its root element cropped
 * to the viewport at the current scroll, within the clip limits, private as
 * every clip is, with what it left out. Fixed and stuck elements are drawn
 * where they show. */
export async function renderViewportClip(win: Window = window, timeoutMs = AREA_CLIP_TIMEOUT_MS): Promise<RenderResult> {
  const root = win.document.documentElement;
  return renderClipWithReport(root, win, timeoutMs, {
    crop: { x: win.scrollX, y: win.scrollY, w: win.innerWidth, h: win.innerHeight },
    style: { margin: "0", display: "flow-root" },
    viewport: true,
  });
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
  return (await renderTargetClipWithReport(t, win, timeoutMs)).png;
}

/** `renderTargetClip`'s PNG, with what it left out. */
export async function renderTargetClipWithReport(t: Element | Range, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS): Promise<RenderResult> {
  if ("nodeType" in t) {
    const r = t.getBoundingClientRect();
    if (fitsClipBudget(r)) return renderClipWithReport(t, win, timeoutMs);
    const doc = t.ownerDocument;
    const vp = { w: doc.documentElement.clientWidth || win.innerWidth, h: doc.documentElement.clientHeight || win.innerHeight };
    const crop = elementRegion(r, vp);
    const whole = crop.x === 0 && crop.y === 0 && crop.w === Math.round(r.width) && crop.h === Math.round(r.height);
    return renderClipWithReport(t, win, timeoutMs, whole ? {} : { crop });
  }
  const range = t;
  const block = blockAncestor(range.commonAncestorContainer, win);
  if (fitsClipBudget(block.getBoundingClientRect())) return renderClipWithReport(block, win, timeoutMs);
  return renderRegionWithReport(block, range, win, timeoutMs);
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

const XHTML = "http://www.w3.org/1999/xhtml";

/** The element an area clip of `el` renders: `el` itself when it is an HTML
 * element other than `<html>`; the body for `<html>` (an area anchored to the
 * document); else (an `<svg>`, MathML) its nearest HTML ancestor, since
 * modern-screenshot draws a foreign element as a whole image and ignores the crop. */
export function areaRenderRoot(el: Element): Element {
  const doc = el.ownerDocument;
  if (el === doc.documentElement) return doc.body;
  let cur: Element = el;
  while (cur.namespaceURI !== XHTML && cur.parentElement) cur = cur.parentElement;
  return cur === doc.documentElement ? doc.body : cur;
}

/** A PNG of exactly the drawn area `r` (viewport pixels) as the page shows
 * it now: `areaRenderRoot(el)` rendered and cropped to `r` (which may reach
 * past it, for an area on the document: that part shows the page's background). */
export async function renderAreaClip(el: Element, r: { x: number; y: number; w: number; h: number }, win: Window = window, timeoutMs = AREA_CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  return (await renderAreaClipWithReport(el, r, win, timeoutMs)).png;
}

/** `renderAreaClip`'s PNG, with what it left out. */
export async function renderAreaClipWithReport(el: Element, r: { x: number; y: number; w: number; h: number }, win: Window = window, timeoutMs = AREA_CLIP_TIMEOUT_MS): Promise<RenderResult> {
  const root = areaRenderRoot(el);
  return renderClipWithReport(root, win, timeoutMs, { crop: areaCrop(r, root.getBoundingClientRect()) });
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
  return (await renderRegionWithReport(block, range, win, timeoutMs)).png;
}

/** `renderRegion`'s PNG, with what it left out: the page's elements in the
 * region the copy cannot draw (embedded content, videos and canvases, which
 * the copy holds as empty placeholders, and images from other origins),
 * measured on the page. */
export async function renderRegionWithReport(block: Element, range: Range, win: Window = window, timeoutMs = CLIP_TIMEOUT_MS): Promise<RenderResult> {
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
    const at: Rect = { x: br.left, y: top, w: br.width, h: Math.max(1, bottom - top) };
    return await renderClipWithReport(copy, win, timeoutMs, { omitted: { at, list: regionOmitted(commonEl ?? block, part, at, new URL(win.location.href).origin) } });
  } finally {
    copy.remove();
  }
}

/** What a region copy of `part` (in `within`) cannot draw within `at`
 * (viewport CSS px): embedded content, videos and canvases, which the copy
 * holds as empty placeholders, and images from other origins; never what a
 * render leaves out or a redaction covers. */
function regionOmitted(within: Element, part: Range, at: Rect, origin: string): Omitted[] {
  const found: Omitted[] = [];
  for (const el of Array.from(within.querySelectorAll("iframe, object, embed, video, canvas, img"))) {
    if (!part.intersectsNode(el) || el.closest(`${UNRENDERED}, [${REDACT_ATTR}]`)) continue;
    const kind: Omitted["kind"] | null = el.localName === "img" ? (crossOriginImage(el, origin) ? "image" : null)
      : el.localName === "video" ? "video" : el.localName === "canvas" ? "canvas" : "iframe";
    const cut = kind && intersect(rectOf(el), at);
    if (kind && cut) found.push({ kind, rect: cut });
  }
  return largest(found);
}
