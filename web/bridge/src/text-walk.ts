// Walking the text a reader sees (not in scripts, styles, templates, or the
// Artifax overlay) outward from one point, without indexing a whole element:
// the finer comment targets (`target.ts`) and region clips (`clip.ts`) work
// from windows of text around the pointer or the picked range.

import { OVERLAY_TAG } from "./anchor";

const SKIP = /^(script|style|noscript|template)$/i;
/** Characters a text window reaches in each direction from its starting point, at most. */
export const WINDOW_CAP = 4000;

const skipped = (n: Node) => n.nodeType === Node.ELEMENT_NODE && (SKIP.test((n as Element).localName) || (n as Element).localName === OVERLAY_TAG);

/** Whether `t` is text a reader sees. */
export function readable(t: Text): boolean {
  const p = t.parentElement;
  return !!p && !SKIP.test(p.localName) && !p.closest(OVERLAY_TAG);
}

/** The node after `n` in document order inside `root`, not entering skipped elements. */
function following(root: Node, n: Node): Node | null {
  if (n.firstChild && !skipped(n)) return n.firstChild;
  for (let c: Node | null = n; c && c !== root; c = c.parentNode) if (c.nextSibling) return c.nextSibling;
  return null;
}

/** The node before `n` in document order inside `root` (its parent before its
 * previous sibling's descendants), not entering skipped elements. */
function preceding(root: Node, n: Node): Node | null {
  if (n === root) return null;
  let p = n.previousSibling;
  if (!p) return n.parentNode === root ? null : n.parentNode;
  while (p.lastChild && !skipped(p)) p = p.lastChild;
  return p;
}

/** The first text node a reader sees after `n` (not inside it when `n` is text) in `root`. */
export function nextText(root: Node, n: Node): Text | null {
  for (let c = following(root, n); c; c = following(root, c)) if (c.nodeType === Node.TEXT_NODE) return c as Text;
  return null;
}

/** The last text node a reader sees before `n` in `root`. */
export function prevText(root: Node, n: Node): Text | null {
  for (let c = preceding(root, n); c; c = preceding(root, c)) if (c.nodeType === Node.TEXT_NODE) return c as Text;
  return null;
}

/** Part of a text node: `node.data.slice(from, to)`. */
export interface Seg { node: Text; from: number; to: number }
/** Text around a point: `text` joins `segs` in document order, and the point is at `at`. */
export interface TextWindow { segs: Seg[]; text: string; at: number }

/** Whether `re` (global) matches in `s` at an index below `limit`. */
function matchBefore(re: RegExp, s: string, limit: number): boolean {
  re.lastIndex = 0;
  const m = re.exec(s);
  return !!m && m.index < limit;
}
const matches = (re: RegExp, s: string) => { re.lastIndex = 0; return re.test(s); };

/** The text in `root` around (`node`, `offset`), reaching in each direction to
 * the first text node holding a match of `stop` (a global pattern) or
 * `WINDOW_CAP` characters, whichever comes first; never beyond `root`. */
export function windowAround(root: Node, node: Text, offset: number, stop: RegExp, cap = WINDOW_CAP): TextWindow {
  const data = node.data;
  const from = Math.max(0, offset - cap);
  const to = Math.min(data.length, offset + cap);
  let before = data.slice(from, offset);
  let after = data.slice(offset, to);
  const head: Seg[] = [];
  const tail: Seg[] = [];
  if (from === 0 && !matchBefore(stop, data.slice(0, offset + 1), offset)) {
    for (let n: Text | null = prevText(root, node); n && before.length < cap; n = prevText(root, n)) {
      const take = Math.min(n.data.length, cap - before.length);
      const s = n.data.slice(n.data.length - take);
      head.unshift({ node: n, from: n.data.length - take, to: n.data.length });
      const hit = matchBefore(stop, s + before.charAt(0), s.length);
      before = s + before;
      if (hit) break;
    }
  }
  if (to === data.length && !matches(stop, after)) {
    for (let n: Text | null = nextText(root, node); n && after.length < cap; n = nextText(root, n)) {
      const take = Math.min(n.data.length, cap - after.length);
      const s = n.data.slice(0, take);
      tail.push({ node: n, from: 0, to: take });
      const hit = matches(stop, after.slice(-1) + s);
      after += s;
      if (hit) break;
    }
  }
  return { segs: [...head, { node, from, to }, ...tail], text: before + after, at: before.length };
}

/** The boundary point at `off` in `w.text`; at a seam between two segments,
 * the end of the first when `atEnd`, else the start of the second. */
export function pointAt(w: TextWindow, off: number, atEnd: boolean): { node: Text; offset: number } {
  let start = 0;
  for (const s of w.segs) {
    const len = s.to - s.from;
    if (off < start + len || (atEnd && off === start + len)) return { node: s.node, offset: s.from + off - start };
    start += len;
  }
  const last = w.segs[w.segs.length - 1];
  return { node: last.node, offset: last.to };
}

/** A range over `w.text` from `start` to `end`, narrowed to exclude surrounding
 * whitespace; null when only whitespace is left. */
export function trimmedRange(w: TextWindow, start: number, end: number): Range | null {
  while (start < end && /\s/.test(w.text[start])) start++;
  while (end > start && /\s/.test(w.text[end - 1])) end--;
  if (end <= start) return null;
  const s = pointAt(w, start, false);
  const e = pointAt(w, end, true);
  const r = s.node.ownerDocument.createRange();
  r.setStart(s.node, s.offset);
  r.setEnd(e.node, e.offset);
  return r;
}
