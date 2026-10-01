// What comment mode targets under the pointer. An element that fits the
// viewport is the target. An oversized one (a whole file in one <pre>, a long
// article in one <div>) would be outlined off-screen and anchored as a whole,
// so the text under the pointer is targeted instead: its line in
// preformatted text, else the block around it, else its sentence.

import { OVERLAY_TAG } from "./anchor";
import { type TextWindow, readable, trimmedRange, windowAround } from "./text-walk";

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

/** A number in a CSS colour function: `n` as is, `n%` as `n / 100 * pct`. */
function num(v: string | undefined, pct = 1): number {
  if (!v || v === "none") return 0;
  return v.endsWith("%") ? (parseFloat(v) / 100) * pct : parseFloat(v);
}
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
/** Relative luminance of linear sRGB channels. */
const linearLum = (r: number, g: number, b: number) => 0.2126 * clamp01(r) + 0.7152 * clamp01(g) + 0.0722 * clamp01(b);
/** CIE lightness (0 to 100) to relative luminance. */
const labLum = (l: number) => (l > 8 ? ((l + 16) / 116) ** 3 : l / 903.3);
function oklabLum(l: number, a: number, b: number): number {
  const L = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const M = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const S = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return linearLum(4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S, -1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S, -0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S);
}

/** Relative luminance and alpha of the CSS colour `c`, from its own syntax
 * (hex, `rgb()`, `oklab()`, `oklch()`, `lab()`, `lch()`, `color()`), else from
 * painting it on a 1×1 canvas (any other syntax a browser knows); null when
 * neither reads it. */
export function colorLuminance(c: string): { lum: number; alpha: number } | null {
  const s = c.trim().toLowerCase();
  if (s === "transparent") return { lum: 1, alpha: 0 };
  const hex = s.match(/^#([0-9a-f]{3,8})$/);
  if (hex && hex[1].length !== 5 && hex[1].length !== 7) {
    const h = hex[1].length <= 4 ? [...hex[1]].map(x => x + x).join("") : hex[1];
    const v = [0, 2, 4, 6].map(i => parseInt(h.slice(i, i + 2) || "ff", 16));
    return { lum: luminance(v), alpha: v[3] / 255 };
  }
  const fn = s.match(/^([a-z-]+)\(([^)]*)\)$/);
  if (fn) {
    const [body, alphaPart] = fn[2].split("/");
    const p = body.trim().split(/[\s,]+/);
    const alpha = alphaPart !== undefined ? clamp01(num(alphaPart.trim(), 1)) : p.length > 3 && fn[1].startsWith("rgb") ? clamp01(num(p[3], 1)) : 1;
    switch (fn[1]) {
      case "rgb": case "rgba": return { lum: luminance(p.slice(0, 3).map(v => num(v, 255))), alpha };
      case "oklab": return { lum: oklabLum(num(p[0]), num(p[1], 0.4), num(p[2], 0.4)), alpha };
      case "oklch": {
        const h = (num(p[2]) * Math.PI) / 180;
        const ch = num(p[1], 0.4);
        return { lum: oklabLum(num(p[0]), ch * Math.cos(h), ch * Math.sin(h)), alpha };
      }
      case "lab": case "lch": return { lum: labLum(num(p[0], 100)), alpha };
      case "color": {
        const [space, r, g, b] = p;
        const v = [r, g, b].map(x => num(x, 1));
        if (space === "srgb-linear") return { lum: linearLum(v[0], v[1], v[2]), alpha };
        if (space === "xyz" || space === "xyz-d65" || space === "xyz-d50") return { lum: clamp01(v[1]), alpha };
        // sRGB and the wider RGB spaces (display-p3, rec2020, …), read as sRGB.
        return { lum: luminance(v.map(x => clamp01(x) * 255)), alpha };
      }
    }
  }
  return paintedLuminance(c);
}

const painted = new Map<string, { lum: number; alpha: number } | null>();
/** `c` painted on a 1×1 canvas and read back (null without canvas support or for no colour). */
function paintedLuminance(c: string): { lum: number; alpha: number } | null {
  if (painted.has(c)) return painted.get(c)!;
  let out: { lum: number; alpha: number } | null = null;
  try {
    if (typeof OffscreenCanvas === "function" && typeof CSS !== "undefined" && CSS.supports("color", c)) {
      const ctx = new OffscreenCanvas(1, 1).getContext("2d", { willReadFrequently: true });
      if (ctx) {
        ctx.fillStyle = c;
        ctx.fillRect(0, 0, 1, 1);
        const [r, g, b, a] = ctx.getImageData(0, 0, 1, 1).data;
        out = { lum: luminance([r, g, b]), alpha: a / 255 };
      }
    }
  } catch { /* unreadable: no colour */ }
  painted.set(c, out);
  return out;
}

/** Outline colours over the CSS colour `bg` (transparent or unreadable counts
 * as white): the comment colour (`--pin`, #ed5439) as the border, which the
 * overlay keeps legible on any ground between a white and a brown hairline,
 * and a tint of it, stronger over dark grounds, of at least 16%. */
export function outlineColors(bg: string): { border: string; tint: string; tintAlpha: number } {
  const c = colorLuminance(bg);
  const lum = c && c.alpha > 0 ? c.lum : 1;
  return lum < 0.4
    ? { border: "#ed5439", tint: "rgba(237, 84, 57, 0.24)", tintAlpha: 0.24 }
    : { border: "#ed5439", tint: "rgba(237, 84, 57, 0.16)", tintAlpha: 0.16 };
}

/** The colour behind `el`: the background of the nearest of it and its
 * ancestors whose background is at least half opaque, else the page canvas
 * for its `color-scheme` (white, or near-black when the page renders dark). */
export function backgroundBehind(el: Element): string {
  const win = el.ownerDocument.defaultView!;
  for (let cur: Element | null = el; cur; cur = cur.parentElement) {
    const bg = win.getComputedStyle(cur).backgroundColor;
    if (bg && (colorLuminance(bg)?.alpha ?? 0) >= 0.5) return bg;
  }
  const scheme = win.getComputedStyle(el.ownerDocument.documentElement).colorScheme ?? "";
  const dark = /\bdark\b/.test(scheme) && (!/\blight\b/.test(scheme) || !!win.matchMedia?.("(prefers-color-scheme: dark)").matches);
  return dark ? "#121212" : "#ffffff";
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

/** Whether `el` generates no block box of its own: inline, inline-block and
 * the other inline-level displays, `contents`, or no computed display. */
const isInline = (el: Element) => {
  const d = el.ownerDocument.defaultView!.getComputedStyle(el).display || "inline";
  return d.startsWith("inline") || d === "contents";
};
const isPre = (el: Element) => /^(pre|break-spaces)/.test(el.ownerDocument.defaultView!.getComputedStyle(el).whiteSpace || "") || el.localName === "pre";

/** Inline elements that are only text styling: inside an oversized block, the
 * line, block, or sentence around them is the target, not the element. */
export const TEXT_INLINE: ReadonlySet<string> = new Set(["span", "code", "em", "strong", "b", "i", "mark", "small", "sub", "sup", "kbd", "samp", "var", "abbr", "cite", "q", "time", "u", "s"]);
/** Interactive and replaced elements: always their own targets when they fit
 * the viewport, as is anything inside one. */
export const ELEMENT_TARGETS = "button, input, select, textarea, a[href], img, svg, video, audio, canvas, iframe, object, [role=button], [contenteditable], label, summary";

/** Whether `el` is text-like inline content (see `TEXT_INLINE`). */
const textInline = (el: Element) => TEXT_INLINE.has(el.localName) && isInline(el);

/** The nearest of `el` and its ancestors matching `ELEMENT_TARGETS` that is
 * not oversized (an editable article is not a control), or null. */
function controlOf(el: Element, vp: Viewport): Element | null {
  for (let c = el.closest(ELEMENT_TARGETS); c; c = c.parentElement?.closest(ELEMENT_TARGETS) ?? null) {
    if (!isOversized(c.getBoundingClientRect(), vp)) return c;
  }
  return null;
}

const LINE_STOP = /\n/g;
const SENTENCE_STOP = /[.!?](?=\s)|\n/g;
const SENTENCE_END = /[.!?](?=\s|$)|\n/g;

/** The line of `w.text` around `w.at`, whitespace trimmed. */
function lineIn(w: TextWindow): Range | null {
  const start = w.text.lastIndexOf("\n", w.at - 1) + 1;
  const next = w.text.indexOf("\n", w.at);
  return trimmedRange(w, start, next < 0 ? w.text.length : next);
}

/** The sentence of `w.text` around `w.at`, whitespace trimmed: from just after
 * the last `.`, `!` or `?` followed by whitespace (or newline) before `w.at`,
 * to the first such mark (or newline) at or after it. */
export function sentenceIn(w: TextWindow): Range | null {
  let start = 0;
  SENTENCE_STOP.lastIndex = 0;
  for (let m = SENTENCE_STOP.exec(w.text); m && m.index < w.at; m = SENTENCE_STOP.exec(w.text)) start = m.index + 1;
  SENTENCE_END.lastIndex = w.at;
  const m = SENTENCE_END.exec(w.text);
  const end = m ? m.index + (m[0] === "\n" ? 0 : 1) : w.text.length;
  return trimmedRange(w, start, end);
}

/** The block box `el` belongs to: `el` itself unless it is inline, else its
 * nearest ancestor that is not (or `stop`). */
function blockOf(el: Element, stop: Element | null = null): Element {
  let b = el;
  while (b !== stop && isInline(b) && b.parentElement) b = b.parentElement;
  return b;
}

/** What comment mode targets for the pointer at (`x`, `y`) over `el`. An
 * element that fits the viewport is the target, unless it is text-like inline
 * content (`TEXT_INLINE`) inside an oversized block: then the nearest
 * enclosing control or replaced element (`ELEMENT_TARGETS`) that is not
 * oversized is the target, and without one the block is treated as the
 * element under the pointer (so the tokens of highlighted code do not each
 * become a target). Over an
 * oversized element the target is the line under the pointer in preformatted
 * text, the block around the text when that block fits, or the sentence around
 * it; with no text under the pointer, the smallest element under it that fits;
 * else the oversized element. Only the text near the pointer is read (at most
 * `WINDOW_CAP` characters each way), never a whole element's. */
export function chooseTarget(doc: Document, el: Element, x: number, y: number, vp: Viewport = viewportOf(doc)): Element | Range {
  if (!isOversized(el.getBoundingClientRect(), vp)) {
    if (!textInline(el)) return el;
    const block = blockOf(el);
    if (block === el || block === doc.body || block === doc.documentElement || !isOversized(block.getBoundingClientRect(), vp)) return el;
    const control = controlOf(el, vp);
    if (control) return control;
    el = block;
  }
  const caret = caretAt(doc, x, y);
  const text = caret && caret.node.nodeType === Node.TEXT_NODE && el.contains(caret.node) ? (caret.node as Text) : null;
  const parent = text?.parentElement;
  if (text && parent && readable(text)) {
    const block = blockOf(parent, el);
    if (isPre(parent) || isPre(block)) {
      const line = lineIn(windowAround(block, text, caret!.offset, LINE_STOP));
      if (line) return line;
    } else if (block !== el && !isOversized(block.getBoundingClientRect(), vp)) {
      return block;
    } else {
      const sentence = sentenceIn(windowAround(block, text, caret!.offset, SENTENCE_STOP));
      if (sentence) return sentence;
    }
  }
  const under = typeof doc.elementsFromPoint === "function" ? doc.elementsFromPoint(x, y) : [];
  for (const e of under) {
    if (e !== el && el.contains(e) && !e.closest(OVERLAY_TAG) && !isOversized(e.getBoundingClientRect(), vp)) return e;
  }
  return el;
}
