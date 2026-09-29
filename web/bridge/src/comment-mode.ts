// Comment mode inside the page: an outline on the hovered target, a pin that
// follows the pointer, a click to pick it, a text selection to pick a range,
// Escape to cancel. The target is the element under the pointer, or, inside
// an oversized element, the text under it (see `target.ts`); the outline is
// clamped to the viewport so all four borders show, in colours judged from
// the background behind the target. A selection that already existed when the mode was
// turned on is not a pick; turning the mode off drops any pending hover. The
// overlay lives in a shadow root on <html>, so it never changes the page's
// body, selectors, or text.

import { OVERLAY_TAG } from "./anchor";
import { backgroundBehind, chooseTarget, outlineBox, outlineColors, rectOf, viewportOf } from "./target";

export interface ModeHooks {
  hover(target: Element | Range | null): void;
  pickElement(el: Element): void;
  pickRange(r: Range): void;
  cancel(): void;
}

const CSS = `:host{all:initial}
.o{position:fixed;box-sizing:border-box;pointer-events:none;border:2px solid var(--ax-border,#c2410c);border-radius:3px;background:var(--ax-tint,rgba(194,65,12,.18));z-index:2147483647;display:none}
.o.flash{animation:f .9s ease-out 2}
.pin{position:fixed;pointer-events:none;width:18px;height:18px;margin:-20px 0 0 4px;border-radius:50% 50% 50% 0;background:#c2410c;box-shadow:0 1px 4px rgba(0,0,0,.3);z-index:2147483647;display:none}
@keyframes f{50%{background:rgba(194,65,12,.4)}}`;

export class CommentMode {
  private on = false;
  private readonly host: HTMLElement;
  private readonly outline: HTMLElement;
  private readonly pin: HTMLElement;
  private suppressClick = false;
  private frame = 0;
  private hovered: Element | Range | null = null;
  private border = "";
  private pointer = { x: 0, y: 0, el: null as Element | null };
  private selectionBefore: Range | null = null;

  constructor(private readonly doc: Document, private readonly hooks: ModeHooks) {
    this.host = doc.createElement(OVERLAY_TAG);
    const root = this.host.attachShadow({ mode: "open" });
    root.innerHTML = `<style>${CSS}</style><div class="o"></div><div class="pin"></div>`;
    this.outline = root.querySelector(".o")!;
    this.pin = root.querySelector(".pin")!;
    doc.documentElement.appendChild(this.host);
  }

  set(on: boolean): void {
    if (on === this.on) return;
    this.on = on;
    const listeners: [string, EventListener][] = [
      ["mousemove", this.onMove as EventListener],
      ["mousedown", this.onDown as EventListener],
      ["mouseup", this.onUp as EventListener],
      ["click", this.onClick as EventListener],
      ["keydown", this.onKey as EventListener],
    ];
    for (const [type, fn] of listeners) {
      if (on) this.doc.addEventListener(type, fn, true);
      else this.doc.removeEventListener(type, fn, true);
    }
    this.doc.documentElement.style.cursor = on ? "crosshair" : "";
    this.suppressClick = false;
    if (on) {
      const sel = this.doc.getSelection();
      this.selectionBefore = sel && !sel.isCollapsed && sel.rangeCount ? sel.getRangeAt(0).cloneRange() : null;
    } else {
      if (this.frame) cancelAnimationFrame(this.frame);
      this.frame = 0;
      this.selectionBefore = null;
      this.outline.style.display = "none";
      this.pin.style.display = "none";
      this.hovered = null;
    }
  }

  /** Outlines `target` briefly (after a scroll-to). */
  flash(target: Element | Range): void {
    this.place(target);
    this.outline.classList.add("flash");
    setTimeout(() => {
      this.outline.classList.remove("flash");
      if (!this.on) this.outline.style.display = "none";
    }, 1800);
  }

  /** Outlines `t`, in colours that stand out on the background behind it. */
  private place(t: Element | Range): void {
    // Created from `<head>`, the overlay precedes `<body>`; it moves last so it
    // stacks above page content at the same z-index.
    const root = this.doc.documentElement;
    if (root.lastElementChild !== this.host) root.appendChild(this.host);
    const el = t instanceof Element ? t : t.commonAncestorContainer.nodeType === Node.ELEMENT_NODE ? (t.commonAncestorContainer as Element) : t.commonAncestorContainer.parentElement;
    const c = outlineColors(el ? backgroundBehind(el) : "#ffffff");
    if (c.border !== this.border) {
      this.border = c.border;
      this.host.style.setProperty("--ax-border", c.border);
      this.host.style.setProperty("--ax-tint", c.tint);
    }
    const b = outlineBox(rectOf(t), viewportOf(this.doc));
    if (!b) { this.outline.style.display = "none"; return; }
    Object.assign(this.outline.style, { display: "block", left: `${b.left}px`, top: `${b.top}px`, width: `${b.width}px`, height: `${b.height}px` });
  }

  private target(e: Event): Element | null {
    const el = e.target as Element | null;
    if (!el || el === this.host || el === this.doc.documentElement || el === this.doc.body) return null;
    return el.closest?.(OVERLAY_TAG) ? null : el;
  }

  /** The comment target for the pointer at (`x`, `y`) over `el`. */
  private choose(el: Element | null, x: number, y: number): Element | Range | null {
    return el ? chooseTarget(this.doc, el, x, y) : null;
  }

  private onMove = (e: MouseEvent) => {
    Object.assign(this.pin.style, { display: "block", left: `${e.clientX}px`, top: `${e.clientY}px` });
    this.pointer = { x: e.clientX, y: e.clientY, el: this.target(e) };
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      if (!this.on) return;
      const t = this.choose(this.pointer.el, this.pointer.x, this.pointer.y);
      if (sameTarget(t, this.hovered)) return;
      this.hovered = t;
      if (t) this.place(t);
      else this.outline.style.display = "none";
      this.hooks.hover(t);
    });
  };

  /** A new press ends any click suppression left by a drag that produced no click. */
  private onDown = () => {
    this.suppressClick = false;
  };

  private onUp = () => {
    const sel = this.doc.getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount) return;
    const r = sel.getRangeAt(0).cloneRange();
    if (!this.doc.body.contains(r.commonAncestorContainer)) return;
    const before = this.selectionBefore;
    if (before && sameRange(before, r)) return; // selected before the mode was on
    this.selectionBefore = null;
    this.suppressClick = true;
    sel.removeAllRanges();
    this.hooks.pickRange(r);
  };

  private onClick = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (this.suppressClick) { this.suppressClick = false; return; }
    const t = this.choose(this.target(e), e.clientX, e.clientY);
    if (t instanceof Element) this.hooks.pickElement(t);
    else if (t) this.hooks.pickRange(t);
  };

  private onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") this.hooks.cancel();
  };
}

const sameTarget = (a: Element | Range | null, b: Element | Range | null) =>
  a === b || (a instanceof Range && b instanceof Range && sameRange(a, b));

const sameRange = (a: Range, b: Range) =>
  a.startContainer === b.startContainer && a.startOffset === b.startOffset && a.endContainer === b.endContainer && a.endOffset === b.endOffset;
