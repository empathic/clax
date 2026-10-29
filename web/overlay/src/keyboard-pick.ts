// Picking an element from the keyboard, in the overlay's Element mode: an
// arrow key outlines an element (first the one at the viewport's centre),
// Down moves past the outlined element and all it holds to the next one, Up
// goes back the way Down came, Right into the outlined element's first
// child, Left out to its parent, and Enter or Space picks it. Keys typed
// into a text field are never taken, nor any with Option, Ctrl or Cmd held
// (Option widens the pointer's pick in comment mode). The outline is comment
// mode's own (`CommentMode.outlineTarget`), and a pointer's hover moves the
// keyboard's place to what it outlines (`follow`).
//
// Keys pressed in the overlay are the overlay's own (its buttons, its
// drawing stage), but for the arrow keys pressed in the mode bar (marked
// `KEYBOARD_PICK_FROM`), where picking starts: the first of them moves focus
// out of the overlay, so Enter then picks rather than pressing a button.

import { OVERLAY_TAG, UNRENDERED } from "../../bridge/src/anchor";

/** The attribute of the overlay's part whose arrow keys start keyboard picking. */
export const KEYBOARD_PICK_FROM = "data-clax-keyboard-pick";

/** What keyboard picking drives: the outline, the pick, and what to announce. */
export interface KeyboardPickHooks {
  outline(t: Element | null): void;
  pick(el: Element): void;
  hover(t: Element | null): void;
}

const SKIPPED = /^(SCRIPT|STYLE|TEMPLATE|NOSCRIPT|LINK|META)$/;

/** Keyboard picking on `doc`, off until `set(true)`. */
export class KeyboardPick {
  private on = false;
  private target: Element | null = null;
  /** The elements Down came from, for Up to go back to. */
  private trail: Element[] = [];
  /** A pick's clip is being taken: no other pick until `done`. */
  private busy = false;

  constructor(private readonly doc: Document, private readonly hooks: KeyboardPickHooks) {}

  set(on: boolean): void {
    if (on === this.on) return;
    this.on = on;
    if (on) this.doc.addEventListener("keydown", this.onKey, true);
    else this.doc.removeEventListener("keydown", this.onKey, true);
    this.target = null;
    this.trail = [];
  }

  /** The pick's clip is taken: a new pick may start. */
  done(): void {
    this.busy = false;
  }

  /** The elements the keyboard may outline, in document order. */
  private all(): Element[] {
    const out: Element[] = [];
    const walk = (el: Element) => {
      for (const c of Array.from(el.children)) {
        if (SKIPPED.test(c.tagName) || c.matches(UNRENDERED)) continue;
        const v = (c as Element & { checkVisibility?: () => boolean }).checkVisibility;
        if (typeof v !== "function" || v.call(c)) out.push(c);
        walk(c);
      }
    };
    if (this.doc.body) walk(this.doc.body);
    return out;
  }

  private readonly onKey = (e: KeyboardEvent) => {
    if (e.altKey || e.metaKey || e.ctrlKey || e.isComposing) return;
    const path = e.composedPath();
    const at = path[0];
    if (at instanceof HTMLElement && (at.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(at.tagName))) return;
    const inOverlay = path.some(n => n instanceof Element && n.localName === OVERLAY_TAG);
    if (inOverlay) {
      const fromBar = path.some(n => n instanceof Element && n.hasAttribute(KEYBOARD_PICK_FROM));
      if (!fromBar || !e.key.startsWith("Arrow")) return;
    }
    if (!this.key(e.key)) return;
    e.preventDefault();
    e.stopPropagation();
    if (inOverlay && at instanceof HTMLElement) at.blur();
  };

  /** A pointer outlined `t`: the keyboard goes on from there. */
  follow(t: Element | Range | null): void {
    if (!this.on || !(t instanceof Element) || t === this.target) return;
    this.target = t;
    this.trail = [];
  }

  /** Moves or picks for `key`; whether it was used. */
  key(key: string): boolean {
    if (!this.on) return false;
    if (key === "Enter" || key === " ") {
      const t = this.target;
      if (!t || !t.isConnected || this.busy) return false;
      this.target = null;
      this.trail = [];
      this.busy = true;
      this.hooks.outline(null);
      this.hooks.pick(t);
      return true;
    }
    if (!/^Arrow(Up|Down|Left|Right)$/.test(key)) return false;
    const all = this.all();
    if (all.length === 0) return true;
    const after = (e: Element) => all.find(x => (e.compareDocumentPosition(x) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0 && !e.contains(x)) ?? null;
    const cur = this.target?.isConnected ? this.target : null;
    let next: Element;
    if (!cur) {
      const w = this.doc.defaultView;
      const mid = w ? this.doc.elementFromPoint?.(w.innerWidth / 2, w.innerHeight / 2) ?? null : null;
      next = mid && all.includes(mid) ? mid : all[0];
      this.trail = [];
    } else if (key === "ArrowDown") {
      const n = after(cur);
      if (n) this.trail.push(cur);
      next = n ?? cur;
    } else if (key === "ArrowUp") {
      const back = this.trail.at(-1);
      if (back && after(back) === cur) { this.trail.pop(); next = back; }
      else next = [...all].reverse().find(e => !e.contains(cur) && (cur.compareDocumentPosition(e) & Node.DOCUMENT_POSITION_PRECEDING) !== 0 && after(e) === cur) ?? cur;
    } else if (key === "ArrowRight") {
      next = all.find(e => e !== cur && cur.contains(e)) ?? cur;
      this.trail = [];
    } else {
      let p = cur.parentElement;
      while (p && p !== this.doc.body && !all.includes(p)) p = p.parentElement;
      next = p && p !== this.doc.body ? p : cur;
      this.trail = [];
    }
    this.target = next;
    next.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    this.hooks.outline(next);
    this.hooks.hover(next);
    return true;
  }
}
