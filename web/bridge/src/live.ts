// The live layers inside the page (spec 2026-10-11-clax-dev-hosted-design
// §16.3): the viewer's pointer and selection reported to the shell
// (`clax:cursor`), anchored by element path and text offset so they land
// right at any width, and other people's pointers, selections and the open
// threads' quotes drawn here. This is the bridge's `live` part, which
// comment mode's part loads on need (`live-hook.ts`): for the first quote
// to highlight, or once someone else is on the artifact.
//
// Another person arrives as a `FramePeer`: an opaque key and a colour, never
// a name or an ID (the page's own scripts run beside this). Pointers are
// unnamed arrows; the page says where it drew each (`clax:peer-at`, viewport
// pixels) and the shell draws the names over the frame. A selection is drawn
// as its kind is: a text range tinted and underlined line by line, an
// element outlined, an area drawn dashed on its element. Quotes are
// highlighted with the CSS Custom Highlight API where the browser has it.
// Everything is drawn in a shadow root on `<html>`, outside `<body>`, so the
// page's selectors, text and element paths are untouched; it takes no
// pointer events.

import { OVERLAY_TAG } from "./anchor";
import { type AreaRect, areaFractions, boxOf, containingElement } from "./area";
import { type BridgeToShell, type FramePeer, LIVE_CURSOR_MS, LIVE_DEPTH, type LivePoint, type LiveSelection, PEERS_MAX } from "./protocol";
import { caretAt } from "./target";

/** The element the live layers draw in. */
export const LIVE_TAG = "clax-live";
/** How far from the pointer (CSS px) the caret may be and still put the
 * pointer on text; farther, the pointer is placed by its element's box. */
const CARET_NEAR = 24;
const KEY = /^k[0-9]{1,6}$/;
const HEX = /^#[0-9a-fA-F]{6}$/;
const MAX_INDEX = 10_000_000;

const fraction = (v: number) => Math.round(Math.min(1, Math.max(0, v)) * 1e4) / 1e4;

const bodyOf = (doc: Document): Element => doc.body ?? doc.documentElement;

/** `el`'s path from `<body>` (child indexes), or null when it is not inside
 * the body or lies deeper than `LIVE_DEPTH`. The body itself is `[]`. */
export function pathOf(doc: Document, el: Element): number[] | null {
  const body = bodyOf(doc);
  const path: number[] = [];
  let cur: Element | null = el;
  while (cur && cur !== body) {
    const up: Element | null = cur.parentElement;
    if (!up) return null;
    path.unshift(Array.prototype.indexOf.call(up.children, cur) as number);
    cur = up;
  }
  return cur === body && path.length <= LIVE_DEPTH ? path : null;
}

/** The element at `path` from `<body>`, or null when the page has none there. */
export function atPath(doc: Document, path: readonly number[]): Element | null {
  let el: Element | null = bodyOf(doc);
  for (const i of path) {
    el = el?.children[i] ?? null;
    if (!el) return null;
  }
  return el;
}

/** The length of `el`'s text before (`node`, `offset`), or -1 when that is not in `el`. */
export function textOffset(el: Element, node: Node, offset: number): number {
  if (!el.contains(node)) return -1;
  try {
    const r = el.ownerDocument.createRange();
    r.selectNodeContents(el);
    r.setEnd(node, offset);
    return r.toString().length;
  } catch {
    return -1;
  }
}

/** The text node and offset `offset` characters into `el`'s text (its end when it is shorter). */
export function locate(el: Element, offset: number): [Text, number] | null {
  const w = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let last: Text | null = null;
  let left = offset;
  for (let n = w.nextNode() as Text | null; n; n = w.nextNode() as Text | null) {
    if (left <= n.data.length) return [n, left];
    left -= n.data.length;
    last = n;
  }
  return last ? [last, last.data.length] : null;
}

const elementOf = (n: Node): Element | null => (n.nodeType === Node.ELEMENT_NODE ? (n as Element) : n.parentElement);

/** A range end (`node`, `offset`) as a live point on its element's text. */
export function pointOf(doc: Document, node: Node, offset: number): LivePoint | null {
  const el = elementOf(node);
  const path = el && pathOf(doc, el);
  if (!el || !path) return null;
  const o = textOffset(el, node, offset);
  return o < 0 ? null : { path, offset: o, x: 0, y: 0 };
}

/** `r` as a live selection, or null when either end is outside the body. */
export function rangeSelection(doc: Document, r: Range): LiveSelection | null {
  if (r.collapsed) return null;
  const start = pointOf(doc, r.startContainer, r.startOffset);
  const end = pointOf(doc, r.endContainer, r.endOffset);
  return start && end ? { kind: "range", start, end } : null;
}

/** `el` as a live selection, or null when it is outside the body. */
export function elementSelection(doc: Document, el: Element): LiveSelection | null {
  const path = pathOf(doc, el === doc.documentElement ? bodyOf(doc) : el);
  return path ? { kind: "element", path } : null;
}

/** The rectangle `r` (viewport pixels) as an area on the element holding it
 * (the body when only the document does). */
export function areaSelection(doc: Document, r: AreaRect): LiveSelection | null {
  let el = containingElement(doc, r);
  if (el === doc.documentElement) el = bodyOf(doc);
  const path = pathOf(doc, el);
  if (!path) return null;
  const f = areaFractions(r, boxOf(el));
  return { kind: "area", path, x: f.x, y: f.y, w: f.w, h: f.h };
}

/** The text caret's box at (`node`, `offset`), or null when it has none. */
function caretBox(node: Node, offset: number): DOMRect | null {
  try {
    const r = node.ownerDocument!.createRange();
    r.setStart(node, offset);
    r.collapse(true);
    const b = r.getClientRects()[0] ?? r.getBoundingClientRect();
    return b && b.height > 0 ? b : null;
  } catch {
    return null;
  }
}

/** The pointer at (`x`, `y`) in the viewport as a live point: on the text
 * under it when the caret lies within `CARET_NEAR` px, else on its element's
 * box; null over nothing of the body. */
export function pointAt(doc: Document, x: number, y: number): LivePoint | null {
  const hit = typeof doc.elementFromPoint === "function" ? doc.elementFromPoint(x, y) : null;
  if (!hit || hit.closest(`${LIVE_TAG}, ${OVERLAY_TAG}`)) return null;
  const el = hit === doc.documentElement ? bodyOf(doc) : hit;
  const path = pathOf(doc, el);
  if (!path) return null;
  const b = el.getBoundingClientRect();
  let offset = -1;
  const c = caretAt(doc, x, y);
  if (c && c.node.nodeType === Node.TEXT_NODE && el.contains(c.node)) {
    const k = caretBox(c.node, c.offset);
    if (k && Math.abs(k.left - x) < CARET_NEAR && y >= k.top - 4 && y <= k.bottom + 4) offset = textOffset(el, c.node, c.offset);
  }
  return { path, offset, x: fraction(b.width ? (x - b.left) / b.width : 0), y: fraction(b.height ? (y - b.top) / b.height : 0) };
}

/** Where live point `p` is now, in page coordinates; null when the page has no element there. */
export function placePoint(doc: Document, p: LivePoint): { x: number; y: number } | null {
  const el = atPath(doc, p.path);
  if (!el) return null;
  const win = doc.defaultView!;
  if (p.offset >= 0) {
    const l = locate(el, p.offset);
    const k = l && caretBox(l[0], l[1]);
    if (k) return { x: k.left + win.scrollX, y: k.top + k.height / 2 + win.scrollY };
  }
  const b = el.getBoundingClientRect();
  return { x: b.left + p.x * b.width + win.scrollX, y: b.top + p.y * b.height + win.scrollY };
}

/** The range a live range selection names now, or null. */
export function rangeOf(doc: Document, s: Extract<LiveSelection, { kind: "range" }>): Range | null {
  const a = atPath(doc, s.start.path);
  const z = atPath(doc, s.end.path);
  const l = a && locate(a, s.start.offset);
  const m = z && locate(z, s.end.offset);
  if (!l || !m) return null;
  try {
    const r = doc.createRange();
    r.setStart(l[0], l[1]);
    r.setEnd(m[0], m[1]);
    return r.collapsed ? null : r;
  } catch {
    return null;
  }
}

const isInt = (v: unknown, max: number): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= max;
const isFraction = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0 && v <= 1;
const isPath = (v: unknown): v is number[] => Array.isArray(v) && v.length <= LIVE_DEPTH && v.every(i => isInt(i, MAX_INDEX));

function acceptPoint(v: unknown): LivePoint | null {
  if (!v || typeof v !== "object") return null;
  const p = v as Record<string, unknown>;
  if (!isPath(p.path) || !(p.offset === -1 || isInt(p.offset, MAX_INDEX)) || !isFraction(p.x) || !isFraction(p.y)) return null;
  return { path: [...p.path], offset: p.offset as number, x: p.x, y: p.y };
}

function acceptSelection(v: unknown): LiveSelection | null {
  if (!v || typeof v !== "object") return null;
  const s = v as Record<string, unknown>;
  if (s.kind === "range") {
    const start = acceptPoint(s.start), end = acceptPoint(s.end);
    return start && end && start.offset >= 0 && end.offset >= 0 ? { kind: "range", start, end } : null;
  }
  if (s.kind === "element") return isPath(s.path) ? { kind: "element", path: [...s.path] } : null;
  if (s.kind === "area") {
    return isPath(s.path) && [s.x, s.y, s.w, s.h].every(isFraction) ? { kind: "area", path: [...s.path], x: s.x as number, y: s.y as number, w: s.w as number, h: s.h as number } : null;
  }
  return null;
}

/** The well-formed peers of a `clax:peers` message, at most `PEERS_MAX`:
 * an opaque key, a `#rrggbb` colour, and a pointer and selection of the
 * shapes above (a malformed one is none). */
export function acceptPeers(v: unknown): FramePeer[] {
  if (!Array.isArray(v)) return [];
  const out: FramePeer[] = [];
  for (const p of v) {
    if (out.length >= PEERS_MAX) break;
    if (!p || typeof p !== "object" || typeof p.key !== "string" || !KEY.test(p.key) || typeof p.color !== "string" || !HEX.test(p.color)) continue;
    out.push({ key: p.key, color: p.color, anchor: acceptPoint(p.anchor), selection: acceptSelection(p.selection) });
  }
  return out;
}

/** The timers the layer uses: the window's, as they were when the bridge loaded. */
export interface Timers { setTimeout(f: () => void, ms: number): unknown; clearTimeout(t: unknown): void }

const CSS = `:host{all:initial}
.c{position:absolute;left:0;top:0;transition:transform .12s linear;will-change:transform}
.c svg{display:block;width:18px;height:18px;filter:drop-shadow(0 1px 1px rgba(0,0,0,.3))}
.s,.e,.a{position:absolute;box-sizing:border-box;border-radius:2px}
.e{border:2px solid var(--c);background:color-mix(in srgb,var(--c) 10%,transparent)}
.a{border:2px dashed var(--c);background:color-mix(in srgb,var(--c) 10%,transparent)}
.s{background:color-mix(in srgb,var(--c) 22%,transparent);border-bottom:2px solid var(--c)}
@media (prefers-reduced-motion:reduce){.c{transition:none}}`;
const ARROW = '<svg viewBox="0 0 18 18" aria-hidden="true"><path d="M2 1.5 15.5 8.2 9.4 9.6 6.6 15.5Z" stroke="#fff" stroke-width="1.4" stroke-linejoin="round"/></svg>';

/** The live layers of one document. */
export class LiveLayer {
  private readonly host: HTMLElement;
  private readonly root: ShadowRoot;
  private readonly marks: HTMLElement;
  private readonly arrows = new Map<string, HTMLElement>();
  /** Where each peer's pointer was drawn, page coordinates. */
  private readonly at = new Map<string, { x: number; y: number }>();
  private peerList: FramePeer[] = [];
  private pointer: { x: number; y: number } | null = null;
  private drag: LiveSelection | null = null;
  private pick: LiveSelection | null = null;
  private last = "";
  private timer: unknown = null;
  private due = false;
  private raf = 0;
  private scrollRaf = 0;
  private readonly off: (() => void)[] = [];

  constructor(private readonly doc: Document, private readonly post: (m: BridgeToShell) => void, private readonly timers: Timers) {
    this.host = doc.createElement(LIVE_TAG);
    this.host.setAttribute("aria-hidden", "true");
    this.host.style.cssText = "all:initial;position:absolute;left:0;top:0;width:0;height:0;overflow:visible;z-index:2147483646;pointer-events:none";
    this.root = this.host.attachShadow({ mode: "open" });
    this.root.innerHTML = `<style>${CSS}</style><div class="m"></div>`;
    this.marks = this.root.querySelector(".m")!;
    const win = doc.defaultView!;
    const on = <K extends string>(t: EventTarget, type: K, f: (e: Event) => void, opts?: AddEventListenerOptions) => {
      t.addEventListener(type, f, opts);
      this.off.push(() => t.removeEventListener(type, f, opts));
    };
    on(win, "pointermove", e => { const p = e as PointerEvent; if (p.isTrusted) { this.pointer = { x: p.clientX, y: p.clientY }; this.report(); } }, { passive: true });
    on(doc, "pointerout", e => { if (!(e as PointerEvent).relatedTarget) { this.pointer = null; this.report(); } });
    on(win, "blur", () => { this.pointer = null; this.report(); });
    on(doc, "selectionchange", () => this.report());
    on(win, "resize", () => this.redraw());
    // The document's scroll moves the drawing with it, so only the pointers'
    // places in the viewport change; an element's scroll moves what is
    // drawn on its content, which is drawn again.
    on(doc, "scroll", e => {
      if (e.target !== doc) this.redraw();
      else if (this.at.size && !this.scrollRaf) this.scrollRaf = requestAnimationFrame(() => { this.scrollRaf = 0; this.sendAt(); });
    }, { passive: true, capture: true });
    if (typeof ResizeObserver === "function" && doc.body) {
      const ro = new ResizeObserver(() => this.redraw());
      ro.observe(doc.body);
      this.off.push(() => ro.disconnect());
    }
  }

  /** The area being drawn in comment mode (viewport pixels), or null when none is. */
  setDrag(r: AreaRect | null): void {
    const next = r ? areaSelection(this.doc, r) : null;
    if (JSON.stringify(next) === JSON.stringify(this.drag)) return;
    this.drag = next;
    this.report();
  }

  /** The last pick made in comment mode, as others see it while its composer is open. */
  setPick(s: LiveSelection | null): void {
    this.pick = s;
    this.report();
  }

  /** The pointer, the selection and the last pick now. */
  private cursor(): { anchor: LivePoint | null; selection: LiveSelection | null; pick: LiveSelection | null } {
    const anchor = this.pointer ? pointAt(this.doc, this.pointer.x, this.pointer.y) : null;
    let selection = this.drag;
    if (!selection) {
      const s = this.doc.getSelection();
      if (s && s.rangeCount && !s.isCollapsed) selection = rangeSelection(this.doc, s.getRangeAt(0));
    }
    return { anchor, selection, pick: this.pick };
  }

  /** Reports the cursor: at once, then at most every `LIVE_CURSOR_MS`, the last change always. */
  private report(): void {
    if (this.timer !== null) { this.due = true; return; }
    this.send();
    const tick = () => {
      this.timer = null;
      if (!this.due) return;
      this.due = false;
      this.send();
      this.timer = this.timers.setTimeout(tick, LIVE_CURSOR_MS);
    };
    this.timer = this.timers.setTimeout(tick, LIVE_CURSOR_MS);
  }

  private send(): void {
    const c = this.cursor();
    const k = JSON.stringify(c);
    if (k === this.last) return;
    this.last = k;
    this.post({ type: "clax:cursor", ...c });
  }

  /** Draws `peers` in place of the last set. */
  peers(peers: FramePeer[]): void {
    this.peerList = peers;
    this.redraw();
  }

  private mount(): void {
    const root = this.doc.documentElement;
    if (root.lastElementChild !== this.host) root.appendChild(this.host);
  }

  private box(cls: string, color: string, x: number, y: number, w: number, h: number): void {
    const d = this.doc.createElement("div");
    d.className = cls;
    d.style.cssText = `--c:${color};left:${x}px;top:${y}px;width:${w}px;height:${h}px`;
    this.marks.appendChild(d);
  }

  /** Draws every peer again, as the page is laid out now. */
  redraw(): void {
    if (this.raf) return;
    this.raf = requestAnimationFrame(() => { this.raf = 0; this.draw(); });
  }

  /** Draws every peer now (`redraw` waits for the next frame). */
  draw(): void {
    if (!this.peerList.length && !this.arrows.size && !this.marks.childElementCount) return;
    this.mount();
    const win = this.doc.defaultView!;
    const sx = win.scrollX, sy = win.scrollY;
    this.marks.replaceChildren();
    this.at.clear();
    const keep = new Set<string>();
    for (const p of this.peerList) {
      const s = p.selection;
      if (s?.kind === "range") {
        const r = rangeOf(this.doc, s);
        if (r) for (const b of Array.from(r.getClientRects())) if (b.width > 0 && b.height > 0) this.box("s", p.color, b.left + sx, b.top + sy, b.width, b.height);
      } else if (s) {
        const el = atPath(this.doc, s.path);
        if (el) {
          const b = el.getBoundingClientRect();
          if (s.kind === "element") this.box("e", p.color, b.left + sx, b.top + sy, b.width, b.height);
          else this.box("a", p.color, b.left + s.x * b.width + sx, b.top + s.y * b.height + sy, s.w * b.width, s.h * b.height);
        }
      }
      const q = p.anchor && placePoint(this.doc, p.anchor);
      if (!q) continue;
      keep.add(p.key);
      this.at.set(p.key, q);
      let a = this.arrows.get(p.key);
      if (!a) {
        a = this.doc.createElement("div");
        a.className = "c";
        a.innerHTML = ARROW;
        this.arrows.set(p.key, a);
        this.root.appendChild(a);
      }
      a.querySelector("path")!.setAttribute("fill", p.color);
      a.style.transform = `translate(${q.x}px, ${q.y - 2}px)`;
    }
    for (const [k, a] of this.arrows) if (!keep.has(k)) { a.remove(); this.arrows.delete(k); }
    this.sendAt();
  }

  private sendAt(): void {
    const win = this.doc.defaultView!;
    const peers = [...this.at].map(([key, q]) => ({ key, x: Math.round(q.x - win.scrollX), y: Math.round(q.y - win.scrollY) }));
    this.post({ type: "clax:peer-at", peers });
  }

  /** Stops reporting and removes the drawing (a draw still due is dropped). */
  destroy(): void {
    for (const f of this.off.splice(0)) f();
    if (this.timer !== null) this.timers.clearTimeout(this.timer);
    cancelAnimationFrame(this.raf);
    cancelAnimationFrame(this.scrollRaf);
    this.raf = this.scrollRaf = 0;
    this.peerList = [];
    this.host.remove();
  }
}

let quoteSheet: CSSStyleSheet | null | undefined;

/** Highlights the open threads' quotes, `ranges`, in `doc` in place of the
 * last set, with the CSS Custom Highlight API where the browser has it. */
export function highlightQuotes(doc: Document, ranges: Range[]): void {
  const hl = (globalThis as { CSS?: { highlights?: Map<string, unknown> } }).CSS?.highlights;
  const H = (globalThis as { Highlight?: new (...r: Range[]) => unknown }).Highlight;
  if (!hl || !H) return;
  if (!ranges.length) { hl.delete("clax-quote"); return; }
  if (quoteSheet === undefined) {
    try {
      quoteSheet = new CSSStyleSheet();
      quoteSheet.replaceSync("::highlight(clax-quote){background-color:#e0532f2e}");
    } catch { quoteSheet = null; /* no constructable sheets: quotes are not highlighted */ }
  }
  if (!quoteSheet) return;
  const sheets = doc.adoptedStyleSheets;
  if (!sheets.includes(quoteSheet)) doc.adoptedStyleSheets = [...sheets, quoteSheet];
  hl.set("clax-quote", new H(...ranges));
}
