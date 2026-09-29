// Anchor creation and re-resolution (spec §9 "Anchors"). Resolution order:
// selector with a matching html_hash ("exact"), the selector alone
// ("selector"), the quote located by prefix and suffix ("quote"), a
// registered custom name ("custom"); otherwise the anchor is detached.

import { type Anchor, type AnchorRect, INDEX_FILE, type ResolveMethod } from "./protocol";
import { sha256Hex } from "./sha256";

export const AFFIX = 32;
export const MAX_QUOTE = 2000;
/** The daemon's limit on selector length, in characters. */
export const MAX_SELECTOR = 1024;
export const OVERLAY_TAG = "artifax-overlay";
const SKIP = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE"]);
const SIMPLE_TAG = /^[A-Za-z][A-Za-z0-9-]*$/;

const isHigh = (c: number) => c >= 0xd800 && c <= 0xdbff;
const isLow = (c: number) => c >= 0xdc00 && c <= 0xdfff;
/** `text.slice(start, end)`, narrowed so it never starts or ends inside a
 * surrogate pair (the daemon's JSON parser rejects lone surrogates). */
function cut(text: string, start: number, end: number): string {
  if (start > 0 && start < text.length && isLow(text.charCodeAt(start)) && isHigh(text.charCodeAt(start - 1))) start++;
  if (end > start && end < text.length && isHigh(text.charCodeAt(end - 1)) && isLow(text.charCodeAt(end))) end--;
  return text.slice(start, Math.max(start, end));
}

interface Piece { node: Text; start: number }
export interface TextIndex { text: string; pieces: Piece[] }
export interface Resolved { method: ResolveMethod; element: Element; range: Range | null }

/** The concatenated data of the text nodes under `root` that a reader sees
 * (not in scripts, styles, or the Artifax overlay), with each node's offset. */
export function textIndex(root: Node): TextIndex {
  const doc = root.ownerDocument ?? (root as Document);
  const walker = doc.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: n => {
      const p = n.parentElement;
      return p && !SKIP.has(p.tagName) && !p.closest(OVERLAY_TAG) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
    },
  });
  const pieces: Piece[] = [];
  let text = "";
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    pieces.push({ node: n as Text, start: text.length });
    text += (n as Text).data;
  }
  return { text, pieces };
}

/** The offset in `idx.text` of the boundary point (`container`, `offset`). */
export function boundaryOffset(idx: TextIndex, container: Node, offset: number): number {
  if (container.nodeType === Node.TEXT_NODE) {
    const p = idx.pieces.find(x => x.node === container);
    if (p) return p.start + Math.min(offset, p.node.data.length);
  }
  const point = container.ownerDocument!.createRange();
  point.setStart(container, offset);
  point.collapse(true);
  for (const p of idx.pieces) if (point.comparePoint(p.node, 0) >= 0) return p.start;
  return idx.text.length;
}

/** A range over `idx.text` from `start` to `end`, or null for a page without text. */
export function rangeAt(idx: TextIndex, start: number, end: number): Range | null {
  if (!idx.pieces.length) return null;
  const locate = (off: number, atEnd: boolean) => {
    for (const p of idx.pieces) {
      const stop = p.start + p.node.data.length;
      if (off < stop || (atEnd && off === stop)) return { node: p.node, offset: off - p.start };
    }
    const lastPiece = idx.pieces[idx.pieces.length - 1];
    return { node: lastPiece.node, offset: lastPiece.node.data.length };
  };
  const s = locate(start, false);
  const e = locate(end, true);
  const r = s.node.ownerDocument!.createRange();
  r.setStart(s.node, s.offset);
  r.setEnd(e.node, e.offset);
  return r;
}

/** A selector from `body` (or the nearest ancestor with a unique, simple ID),
 * adding `:nth-of-type(k)` only where a parent has several children of the tag.
 * Element names keep their case (`linearGradient`), since selectors match
 * non-HTML names case-sensitively. A name that is not plain ASCII, or one too
 * long to fit within `MAX_SELECTOR`, becomes `:nth-child(k)`, so the selector never
 * holds control characters or line separators. A path longer than
 * `MAX_SELECTOR` keeps only its trailing steps (best effort: it may then match
 * an earlier element too). */
export function cssPath(el: Element): string {
  const full = fullPath(el);
  if (full.length <= MAX_SELECTOR) return full;
  const steps = full.split(" > ");
  while (steps.length > 1 && steps.join(" > ").length > MAX_SELECTOR) steps.shift();
  return steps.join(" > ");
}

function fullPath(el: Element): string {
  const doc = el.ownerDocument;
  const parts: string[] = [];
  let cur: Element | null = el;
  while (cur && cur !== doc.body && cur !== doc.documentElement) {
    if (cur.id && cur.id.length < MAX_SELECTOR && /^[A-Za-z][\w-]*$/.test(cur.id) && doc.querySelectorAll(`#${cur.id}`).length === 1) {
      parts.unshift(`#${cur.id}`);
      return parts.join(" > ");
    }
    const tag = cur.localName;
    const parent: Element | null = cur.parentElement;
    let step = tag;
    if (!SIMPLE_TAG.test(tag) || tag.length > MAX_SELECTOR - 32) {
      step = `:nth-child(${(parent ? Array.from(parent.children).indexOf(cur) : 0) + 1})`;
    } else if (parent) {
      const same = Array.from(parent.children).filter(c => c.localName === cur!.localName && c.namespaceURI === cur!.namespaceURI);
      if (same.length > 1) step += `:nth-of-type(${same.indexOf(cur) + 1})`;
    }
    parts.unshift(step);
    cur = parent;
  }
  return parts.length ? `body > ${parts.join(" > ")}` : "body";
}

/** `sha256:<hex>` of `el`'s outer HTML. */
export const htmlHash = (el: Element) => `sha256:${sha256Hex(el.outerHTML)}`;

function anchorRect(target: Element | Range, win: Window): AnchorRect {
  const r = typeof target.getBoundingClientRect === "function" ? target.getBoundingClientRect() : null;
  return { x: r?.x ?? 0, y: r?.y ?? 0, w: r?.width ?? 0, h: r?.height ?? 0, scrollX: win.scrollX, scrollY: win.scrollY, viewportW: win.innerWidth };
}

function span(idx: TextIndex, el: Element): [number, number] | null {
  const inside = idx.pieces.filter(p => el.contains(p.node));
  if (!inside.length) return null;
  const lastPiece = inside[inside.length - 1];
  return [inside[0].start, lastPiece.start + lastPiece.node.data.length];
}

function affixes(idx: TextIndex, start: number, quote: string) {
  return { prefix: cut(idx.text, Math.max(0, start - AFFIX), start), suffix: cut(idx.text, start + quote.length, start + quote.length + AFFIX) };
}

/** An anchor on `el` of the page published at `file`. */
export function buildElementAnchor(doc: Document, el: Element, file: string = INDEX_FILE): Anchor {
  const idx = textIndex(doc.body);
  const s = span(idx, el);
  let quote: string | null = null;
  let prefix: string | null = null;
  let suffix: string | null = null;
  if (s && idx.text.slice(s[0], s[1]).trim()) {
    quote = cut(idx.text, s[0], Math.min(s[1], s[0] + MAX_QUOTE));
    ({ prefix, suffix } = affixes(idx, s[0], quote));
  }
  return { kind: "element", selector: cssPath(el), quote, prefix, suffix, html_hash: htmlHash(el), rect: anchorRect(el, doc.defaultView!), custom_name: null, file };
}

/** An anchor on `range` of the page published at `file`. */
export function buildRangeAnchor(doc: Document, range: Range, file: string = INDEX_FILE): Anchor {
  const idx = textIndex(doc.body);
  const start = boundaryOffset(idx, range.startContainer, range.startOffset);
  const end = Math.max(start, boundaryOffset(idx, range.endContainer, range.endOffset));
  const quote = cut(idx.text, start, Math.min(end, start + MAX_QUOTE));
  const c = range.commonAncestorContainer;
  const el = c.nodeType === Node.ELEMENT_NODE ? (c as Element) : c.parentElement!;
  return { kind: "range", selector: cssPath(el), quote, ...affixes(idx, start, quote), html_hash: htmlHash(el), rect: anchorRect(range, doc.defaultView!), custom_name: null, file };
}

const commonSuffix = (a: string, b: string) => { let n = 0; while (n < a.length && n < b.length && a[a.length - 1 - n] === b[b.length - 1 - n]) n++; return n; };
const commonPrefix = (a: string, b: string) => { let n = 0; while (n < a.length && n < b.length && a[n] === b[n]) n++; return n; };

/** The occurrence of `quote` in `idx.text` (inside `within` when given) whose
 * surroundings best match `prefix` and `suffix`; the first on a tie. */
export function findQuote(idx: TextIndex, quote: string, prefix: string, suffix: string, within?: [number, number]): [number, number] | null {
  if (!quote) return null;
  const [lo, hi] = within ?? [0, idx.text.length];
  let best: [number, number] | null = null;
  let bestScore = -1;
  for (let i = idx.text.indexOf(quote, lo); i !== -1 && i + quote.length <= hi; i = idx.text.indexOf(quote, i + 1)) {
    const score = commonSuffix(idx.text.slice(Math.max(0, i - prefix.length), i), prefix)
      + commonPrefix(idx.text.slice(i + quote.length, i + quote.length + suffix.length), suffix);
    if (score > bestScore) { best = [i, i + quote.length]; bestScore = score; }
  }
  return best;
}

function query(doc: Document, selector: string): Element | null {
  try { return doc.querySelector(selector); } catch { return null; }
}

/** Characters of an element's text an area fingerprint keeps. */
export const FINGERPRINT_TEXT = 32;

/** The first `FINGERPRINT_TEXT` characters of the text a reader sees in `el`,
 * whitespace collapsed and trimmed; only as much text is read as that needs. */
export function textPrefix(el: Element): string {
  const walker = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
    acceptNode: n => {
      const p = n.parentElement;
      return p && !SKIP.has(p.tagName) && !p.closest(OVERLAY_TAG) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
    },
  });
  let out = "";
  for (let n = walker.nextNode(); n && out.length < FINGERPRINT_TEXT + 1; n = walker.nextNode()) {
    out = `${out} ${(n as Text).data.slice(0, 4000)}`.replace(/\p{Cc}/gu, " ").replace(/\s+/g, " ").trimStart();
  }
  return cut(out.trim(), 0, FINGERPRINT_TEXT);
}

/** An area's element fingerprint: its local name, text prefix, and child
 * element count. */
export function fingerprint(el: Element): { tag: string; text: string; children: number } {
  return { tag: el.localName.slice(0, 64), text: textPrefix(el), children: el.childElementCount };
}

/** Whether `el` matches area anchor `a`'s fingerprint: the same tag, and the
 * same text prefix (its text now starts with the recorded prefix) when the
 * element had text at draw time, else the same
 * child count. Holds when no fingerprint was recorded. */
export function fingerprintHolds(a: Anchor, el: Element): boolean {
  const f = a.area;
  if (!f || f.tag === undefined) return true;
  if (el.localName.slice(0, 64) !== f.tag) return false;
  // Text added after a short recorded prefix keeps the match.
  if (f.text) return textPrefix(el).startsWith(f.text);
  return f.children === undefined || el.childElementCount === f.children;
}

/** How far, as a share, an area's element may have changed width since the
 * area was drawn and still be taken for the same element when only its
 * selector matched. */
export const AREA_WIDTH_SLACK = 0.25;

/** Whether `el`'s width is within `AREA_WIDTH_SLACK` of the width area anchor
 * `a`'s element had at draw time (`rect.w / area.w`). Holds when that is not
 * known, for the document (`html`), and when the viewport's width changed by
 * more than 5% since (a responsive element may then legitimately differ). */
export function areaWidthHolds(a: Anchor, el: Element, doc: Document): boolean {
  if (!a.area || !a.rect || !(a.area.w > 0) || el === doc.documentElement) return true;
  const vw = doc.defaultView?.innerWidth ?? 0;
  if (!a.rect.viewportW || Math.abs(vw - a.rect.viewportW) > 0.05 * a.rect.viewportW) return true;
  const then = a.rect.w / a.area.w;
  const now = el.getBoundingClientRect().width;
  return then <= 0 || Math.abs(now - then) <= AREA_WIDTH_SLACK * then;
}

/** Where `a` is in `doc`, the page published at `file`; null when it is
 * detached, or when it is on another page (its `file` differs). */
export function resolveAnchor(doc: Document, a: Anchor, custom: Map<string, Element> = new Map(), file: string = INDEX_FILE): Resolved | null {
  if ((a.file || INDEX_FILE) !== file) return null;
  if (a.kind === "custom") {
    const el = a.custom_name ? custom.get(a.custom_name) : undefined;
    return el ? { method: "custom", element: el, range: null } : null;
  }
  const idx = textIndex(doc.body);
  const el = a.selector ? query(doc, a.selector) : null;
  if (el) {
    const method: ResolveMethod = a.html_hash && htmlHash(el) === a.html_hash ? "exact" : "selector";
    // An area has no text to confirm a selector-only match: one on an element
    // whose width moved away from its width at draw time is on other content.
    if (method === "selector" && a.kind === "area" && el !== doc.documentElement && (!areaWidthHolds(a, el, doc) || !fingerprintHolds(a, el))) return null;
    let range: Range | null = null;
    if (a.kind === "range" && a.quote) {
      const s = span(idx, el);
      const hit = s && findQuote(idx, a.quote, a.prefix ?? "", a.suffix ?? "", s);
      range = hit ? rangeAt(idx, hit[0], hit[1]) : null;
    }
    return { method, element: el, range };
  }
  if (a.quote) {
    const hit = findQuote(idx, a.quote, a.prefix ?? "", a.suffix ?? "");
    const range = hit && rangeAt(idx, hit[0], hit[1]);
    if (range) {
      const c = range.commonAncestorContainer;
      const element = c.nodeType === Node.ELEMENT_NODE ? (c as Element) : c.parentElement!;
      return { method: "quote", element, range: a.kind === "range" ? range : null };
    }
  }
  return null;
}

/** Resolutions of the shell's anchors by thread ID, kept until the DOM under
 * the resolved element changes (a detached anchor is retried after any change
 * under `body`) or `reset` is called, so scroll and resize only re-measure. */
export class AnchorCache {
  private readonly entries = new Map<string, { anchor: Anchor; res: Resolved | null }>();
  private readonly observer: MutationObserver;

  constructor(private readonly doc: Document, private readonly custom: Map<string, Element> = new Map(), private readonly file: string = INDEX_FILE) {
    const win = doc.defaultView!;
    this.observer = new win.MutationObserver(records => this.invalidate(records));
    this.observer.observe(doc.body, { subtree: true, childList: true, characterData: true, attributes: true });
  }

  resolve(id: string, anchor: Anchor): Resolved | null {
    const hit = this.entries.get(id);
    if (hit && hit.anchor === anchor) return hit.res;
    const res = resolveAnchor(this.doc, anchor, this.custom, this.file);
    this.entries.set(id, { anchor, res });
    return res;
  }

  reset(): void {
    this.entries.clear();
  }

  disconnect(): void {
    this.observer.disconnect();
    this.entries.clear();
  }

  private invalidate(records: MutationRecord[]): void {
    for (const [id, { res }] of this.entries) {
      const el = res?.element;
      if (!el || !el.isConnected || records.some(r => el.contains(r.target))) this.entries.delete(id);
    }
  }
}
