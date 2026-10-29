// What the overlay captures for a report (spec 2026-10-29-feedback-loop
// §4.1–4.2): a pick (Element, which may be a text range, or Area) or the
// visible viewport (Screen), first as a private render of the page's DOM
// (`clip.ts`), which needs no permission; then, if the reporter asks, as the
// browser's exact screenshot of this tab (`display-capture.ts`), cropped to
// the pick. The exact screenshot is suggested when the DOM render left out
// something under what was captured (a cross-origin frame, a canvas, a
// video). The overlay's own host, and whatever the page marked
// `data-clax-ignore` (in open shadow roots too), are stepped aside while the
// browser captures, so they are never in the picture. Another Clax overlay on
// the page (the extension's) is left alone: it guards its own host's style.

import { IGNORE_ATTR } from "../../bridge/src/anchor";
import { type Omitted, type Rect, type RenderResult, intersect, renderViewportClip, toImage } from "../../bridge/src/clip";
import { type DisplayResult, captureDisplay, displayCaptureSupported } from "../../bridge/src/display-capture";
import { nextFrame } from "../../bridge/src/frame";
import type { CaptureInfo } from "../../sdk/src/types";
import type { Picked } from "./picker";

/** What the reporter captures: a pick, a drawn area, or the visible viewport. */
export type Mode = "element" | "area" | "screen";

/** A capture: its PNG (null when none could be made), how it was made, why
 * the one asked for could not be made (`error`; a fallback may still be in
 * `png`), what the reporter should know about it (`note`), whether the
 * exact screenshot would show more (`suggestExact`), where the pick shows
 * in the image (`pick`, in the image's own pixels; null for Screen and for
 * the exact screenshot, which is cropped to the pick), the image's pixels
 * per CSS pixel (`scale`; for the exact screenshot, read from the PNG's own
 * width, so a frame shrunk to fit is measured as it is), and the part of
 * the page the image shows (`at`, in page CSS pixels: the viewport's plus
 * its scroll at capture; null when there is no image), by which marks are
 * moved from one capture of the page to another, and the PNG's own size
 * (`size`, from its header; null when there is none), which the marks are
 * kept within. */
export interface Captured {
  png: Blob | null;
  info: CaptureInfo;
  error: string | null;
  note: string | null;
  suggestExact: boolean;
  pick: Rect | null;
  scale: number;
  at: Rect | null;
  size: { w: number; h: number } | null;
}

/** A PNG's width and height from its header, or null when it is not one. */
export function pngSize(b: ArrayBuffer): { w: number; h: number } | null {
  if (b.byteLength < 24) return null;
  const v = new DataView(b);
  if (v.getUint32(0) !== 0x89504e47 || v.getUint32(12) !== 0x49484452) return null;
  return { w: v.getUint32(16), h: v.getUint32(20) };
}

/** What a capture uses, each replaceable: the viewport render, the display
 * capture, whether the browser can capture the tab, the overlay's own host
 * and more elements to step aside while it does (besides the page's
 * `data-clax-ignore` ones), the wait for the page to be drawn without them,
 * and, for `captureExact`, the DOM capture already made, to keep when the
 * display capture fails. */
export interface CaptureDeps {
  renderViewport?: (win: Window) => Promise<RenderResult>;
  display?: (win: Window, crop?: Rect) => Promise<DisplayResult>;
  supported?: boolean;
  host?: Element;
  aside?: Element[];
  painted?: () => Promise<void>;
  fallback?: Captured;
}

const FALLBACK_MESSAGES: Record<Exclude<DisplayResult, { ok: true }>["reason"], string> = {
  denied: "You cancelled screen sharing — the page render is used instead.",
  other_surface: "That was another tab or window — the page render is used instead.",
  unsupported: "This browser cannot capture the tab — the page render is used instead.",
  failed: "The tab could not be captured — the page render is used instead.",
};

/** Why a pick out of the viewport has no exact screenshot. */
export const OUT_OF_VIEW = "The pick is out of view — the page render is used instead.";

/** Said of an exact screenshot the browser could not confirm was this tab. */
export const UNCONFIRMED_TAB = "Your browser cannot confirm this is this tab — check the picture before you send it.";

const png = (b: ArrayBuffer) => new Blob([b], { type: "image/png" });
const viewportOf = (win: Window): Rect => ({ x: 0, y: 0, w: win.innerWidth, h: win.innerHeight });
const dprOf = (win: Window): number => (Number.isFinite(win.devicePixelRatio) && win.devicePixelRatio > 0 ? win.devicePixelRatio : 1);

/** The capture mode a report records for `mode` and its pick. */
function infoMode(mode: Mode, target: Picked | null): CaptureInfo["mode"] {
  return mode === "screen" || !target ? "screen" : target.mode;
}

function needTarget(mode: Mode, target: Picked | null): void {
  if (mode !== "screen" && !target) throw new TypeError(`a ${mode} capture needs a pick`);
}

/** The DOM render for `mode`: the pick's clip, or the viewport's. Never
 * rejects: a render that fails gives no PNG and its reason. The exact
 * screenshot is suggested (where the browser can take one) when something
 * the render left out lies under the pick, or the render failed. */
export async function captureFor(mode: Mode, target: Picked | null, win: Window, deps: CaptureDeps = {}): Promise<Captured> {
  needTarget(mode, target);
  const supported = deps.supported ?? displayCaptureSupported(win);
  const m = infoMode(mode, target);
  const scroll = { x: win.scrollX || 0, y: win.scrollY || 0 };
  try {
    const res = mode === "screen" || !target ? await (deps.renderViewport ?? renderViewportClip)(win) : await target.render();
    // What was captured, in the image's pixels (where `omitted` is): a pick may be
    // only part of its clip (a range in its block, an element in its region).
    const whole = toImage(res.at, res.at, res.scale);
    const under = mode === "screen" || !target ? whole : toImage(target.rect(), res.at, res.scale);
    const hidden = res.omitted.some((o: Omitted) => intersect(o.rect, under));
    return { png: png(res.png), info: { mode: m, method: "dom", omitted: res.omitted }, error: null, note: null, suggestExact: supported && hidden, pick: mode === "screen" || !target ? null : under, scale: res.scale, at: { ...res.at, x: res.at.x + scroll.x, y: res.at.y + scroll.y }, size: pngSize(res.png) };
  } catch (e) {
    const error = e instanceof Error && e.message ? e.message : String(e);
    return { png: null, info: { mode: m, method: "dom" }, error, note: null, suggestExact: supported, pick: null, scale: dprOf(win), at: null, size: null };
  }
}

const twoFrames = () => new Promise<void>(resolve => nextFrame(() => nextFrame(resolve)));

/** The page's elements marked `data-clax-ignore`, through open shadow roots. */
export function ignoredElements(doc: Document): Element[] {
  const out: Element[] = [];
  const visit = (root: Document | ShadowRoot) => {
    for (const el of Array.from(root.querySelectorAll("*"))) {
      if (el.hasAttribute(IGNORE_ATTR)) out.push(el);
      if (el.shadowRoot) visit(el.shadowRoot);
    }
  };
  visit(doc);
  return out;
}

/** Runs `capture` with `els` made fully transparent (`opacity: 0
 * !important`: their layout kept, and nothing they hold can show through)
 * once the page has been drawn without them, and puts back each one's own
 * opacity, value and priority, whatever comes of it. */
export async function steppingAside<T>(els: Element[], capture: () => Promise<T>, painted: () => Promise<void> = twoFrames): Promise<T> {
  const prior = Array.from(new Set(els)).map(el => {
    const s = (el as HTMLElement).style;
    return s ? { s, value: s.getPropertyValue("opacity"), priority: s.getPropertyPriority("opacity") } : null;
  });
  for (const p of prior) p?.s.setProperty("opacity", "0", "important");
  try {
    await painted();
    return await capture();
  } finally {
    for (const p of prior) {
      if (!p) continue;
      if (p.value) p.s.setProperty("opacity", p.value, p.priority); else p.s.removeProperty("opacity");
    }
  }
}

/** The browser's screenshot of this tab for `mode`, cropped to the pick
 * (cut to the viewport) for Element and Area. When it cannot be had, the
 * DOM capture (`deps.fallback`, else made now) is answered with `error`
 * saying why; one the browser cannot confirm is this tab carries a `note`. */
export async function captureExact(mode: Mode, target: Picked | null, win: Window, deps: CaptureDeps = {}): Promise<Captured> {
  needTarget(mode, target);
  const m = infoMode(mode, target);
  const crop = mode === "screen" || !target ? undefined : intersect(target.rect(), viewportOf(win));
  let error: string;
  if (crop === null) {
    error = OUT_OF_VIEW;
  } else {
    const aside = [...(deps.host ? [deps.host] : []), ...ignoredElements(win.document), ...(deps.aside ?? [])];
    const display = deps.display ?? ((w: Window, c?: Rect) => captureDisplay(w, c));
    const shown = crop ?? viewportOf(win);
    const scroll = { x: win.scrollX || 0, y: win.scrollY || 0 };
    const res = await steppingAside(aside, () => display(win, crop), deps.painted);
    if (res.ok) {
      const size = pngSize(res.png);
      const scale = size && shown.w > 0 ? size.w / shown.w : dprOf(win);
      return { png: png(res.png), info: { mode: m, method: "display" }, error: null, note: res.thisTab === true ? null : UNCONFIRMED_TAB, suggestExact: false, pick: null, scale, at: { ...shown, x: shown.x + scroll.x, y: shown.y + scroll.y }, size };
    }
    error = FALLBACK_MESSAGES[res.reason];
  }
  const dom = deps.fallback ?? await captureFor(mode, target, win, deps);
  return { ...dom, error, suggestExact: false };
}
