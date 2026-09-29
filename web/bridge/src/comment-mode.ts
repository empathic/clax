// Comment mode inside the page: an outline on the hovered target, a pin that
// follows the pointer, a click to pick it, a text selection to pick a range,
// a drawn rectangle to pick an area, Escape to cancel. The target is the
// element under the pointer, or, inside an oversized element, the text under
// it (see `target.ts`); while Option (Alt) is held it is the enclosing
// element instead, one ancestor further per Up press and back per Down
// (`area.ts`). A drag that starts where no text is under the pointer, or any
// drag with Shift held, draws a rectangle (shown live, clamped to the
// viewport); one smaller than `AREA_MIN` each way is a click, and Escape
// drops it. The outline is clamped to the viewport so all four borders show,
// in colours judged from the background behind the target. A selection that
// already existed when the mode was turned on is not a pick; turning the
// mode off drops any pending hover, drag, and widening. The overlay lives in
// a shadow root on <html>, so it never changes the page's body, selectors, or
// text; it also draws the dashed outline of a focused thread's area.

import { OVERLAY_TAG } from "./anchor";
import { type AreaRect, Widen, dragRect, isClickSized, nonTextAt, widenedTarget } from "./area";
import type { Box } from "./protocol";
import { backgroundBehind, chooseTarget, outlineBox, outlineColors, rectOf, viewportOf } from "./target";

export interface ModeHooks {
  hover(target: Element | Range | null): void;
  pickElement(el: Element): void;
  pickRange(r: Range): void;
  /** A rectangle drawn in the viewport, at least `AREA_MIN` px one way. */
  pickArea(r: AreaRect): void;
  cancel(): void;
}

const CSS = `:host{all:initial}
.o,.a,.f{position:fixed;box-sizing:border-box;pointer-events:none;border:2px solid var(--ax-border,#c2410c);border-radius:3px;background:var(--ax-tint,rgba(194,65,12,.18));z-index:2147483647;display:none}
.a{border-radius:0}
.f{border-style:dashed;border-radius:0;background:none}
.o.flash,.f.flash{animation:f .9s ease-out 2}
.pin{position:fixed;pointer-events:none;width:18px;height:18px;margin:-20px 0 0 4px;border-radius:50% 50% 50% 0;background:#c2410c;box-shadow:0 1px 4px rgba(0,0,0,.3);z-index:2147483647;display:none}
@keyframes f{50%{background:rgba(194,65,12,.4)}}`;

/** A drag that draws an area: where it started, in page coordinates. */
type Drag = { x: number; y: number };

export class CommentMode {
  private on = false;
  private readonly host: HTMLElement;
  private readonly outline: HTMLElement;
  private readonly areaBox: HTMLElement;
  private readonly focusBox: HTMLElement;
  private readonly pin: HTMLElement;
  private suppressClick = false;
  private frame = 0;
  private hovered: Element | Range | null = null;
  /** The target under the pointer before widening. */
  private base: Element | Range | null = null;
  private readonly widen = new Widen();
  private drag: Drag | null = null;
  private border = "";
  private pointer = { x: 0, y: 0, el: null as Element | null };
  private selectionBefore: Range | null = null;

  constructor(private readonly doc: Document, private readonly hooks: ModeHooks) {
    this.host = doc.createElement(OVERLAY_TAG);
    const root = this.host.attachShadow({ mode: "open" });
    root.innerHTML = `<style>${CSS}</style><div class="f"></div><div class="o"></div><div class="a"></div><div class="pin"></div>`;
    this.outline = root.querySelector(".o")!;
    this.areaBox = root.querySelector(".a")!;
    this.focusBox = root.querySelector(".f")!;
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
      ["keyup", this.onKeyUp as EventListener],
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
      this.base = null;
      this.widen.stop();
      this.endDrag();
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

  /** Draws the dashed outline of a thread's area at `box` (viewport pixels;
   * clamped like the hover outline), or hides it for null or a box out of
   * view; `flash` pulses it (after a scroll-to). */
  showFocus(box: Box | null, flash = false): void {
    const b = box && outlineBox({ left: box.x, top: box.y, right: box.x + box.w, bottom: box.y + box.h, width: box.w, height: box.h }, viewportOf(this.doc));
    if (!b) { this.focusBox.style.display = "none"; return; }
    Object.assign(this.focusBox.style, { display: "block", left: `${b.left}px`, top: `${b.top}px`, width: `${b.width}px`, height: `${b.height}px` });
    if (flash) {
      this.focusBox.classList.add("flash");
      setTimeout(() => this.focusBox.classList.remove("flash"), 1800);
    }
  }

  /** A key pressed (`down`) or released in the shell while the pointer is
   * over the frame, or in the page: Option widens, Up and Down move the
   * widening while it is on. Whether the key was used (an arrow key's
   * default, scrolling, is then prevented). */
  key(key: string, down: boolean): boolean {
    if (!this.on) return false;
    if (key === "Alt") { this.setAlt(down); return false; }
    if (!down || !this.widen.active) return false;
    if (key === "ArrowUp") this.widen.up();
    else if (key === "ArrowDown") this.widen.down();
    else return false;
    this.refresh();
    return true;
  }

  private setAlt(on: boolean): void {
    if (on === this.widen.active) return;
    if (on) this.widen.start();
    else this.widen.stop();
    this.refresh();
  }

  /** The target for `base` under the current widening (clamping it). */
  private widened(base: Element | Range | null): Element | Range | null {
    if (!base || !this.widen.active) return base;
    const w = widenedTarget(base, this.widen.level);
    this.widen.clamp(w.level);
    return w.target;
  }

  /** Re-outlines and reports the target after the widening changed. */
  private refresh(): void {
    if (!this.on || this.drag) return;
    const t = this.widened(this.base);
    if (sameTarget(t, this.hovered)) return;
    this.hovered = t;
    if (t) this.place(t);
    else this.outline.style.display = "none";
    this.hooks.hover(t);
  }

  /** Sets the overlay's colours for the background behind `el`. */
  private colour(el: Element | null): void {
    const c = outlineColors(el ? backgroundBehind(el) : "#ffffff");
    if (c.border !== this.border) {
      this.border = c.border;
      this.host.style.setProperty("--ax-border", c.border);
      this.host.style.setProperty("--ax-tint", c.tint);
    }
  }

  /** Outlines `t`, in colours that stand out on the background behind it. */
  private place(t: Element | Range): void {
    this.colour(t instanceof Element ? t : t.commonAncestorContainer.nodeType === Node.ELEMENT_NODE ? (t.commonAncestorContainer as Element) : t.commonAncestorContainer.parentElement);
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

  /** The drag's rectangle with the pointer at (`x`, `y`), in the viewport. */
  private dragRectTo(x: number, y: number): AreaRect {
    const win = this.doc.defaultView!;
    const d = this.drag!;
    return dragRect({ x: d.x - win.scrollX, y: d.y - win.scrollY }, { x, y }, viewportOf(this.doc));
  }

  private endDrag(): void {
    this.drag = null;
    this.areaBox.style.display = "none";
  }

  private onMove = (e: MouseEvent) => {
    Object.assign(this.pin.style, { display: "block", left: `${e.clientX}px`, top: `${e.clientY}px` });
    if (this.drag) {
      // The button came up outside the frame: the drag is dropped.
      if (!(e.buttons & 1)) { this.endDrag(); return; }
      const b = outlineBox(rectOf0(this.dragRectTo(e.clientX, e.clientY)), viewportOf(this.doc));
      if (b) Object.assign(this.areaBox.style, { display: "block", left: `${b.left}px`, top: `${b.top}px`, width: `${b.width}px`, height: `${b.height}px` });
      this.outline.style.display = "none";
      return;
    }
    // Option held or released while focus was elsewhere shows on the pointer.
    if (e.altKey !== this.widen.active) {
      if (e.altKey) this.widen.start();
      else this.widen.stop();
    }
    this.pointer = { x: e.clientX, y: e.clientY, el: this.target(e) };
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      if (!this.on || this.drag) return;
      this.base = this.choose(this.pointer.el, this.pointer.x, this.pointer.y);
      const t = this.widened(this.base);
      if (sameTarget(t, this.hovered)) return;
      this.hovered = t;
      if (t) this.place(t);
      else this.outline.style.display = "none";
      this.hooks.hover(t);
    });
  };

  /** A new press ends any click suppression left by a drag that produced no
   * click, and starts drawing an area when Shift is held or no text is under
   * the pointer (the press's default, starting a selection or dragging an
   * image, is then prevented). */
  private onDown = (e: MouseEvent) => {
    this.suppressClick = false;
    this.endDrag();
    if (e.button !== 0 || this.widen.active) return;
    const el = this.target(e);
    if (!e.shiftKey && !nonTextAt(this.doc, el, e.clientX, e.clientY)) return;
    e.preventDefault();
    const win = this.doc.defaultView!;
    this.drag = { x: e.clientX + win.scrollX, y: e.clientY + win.scrollY };
    this.colour(el ?? this.doc.body);
  };

  private onUp = (e: MouseEvent) => {
    if (this.drag) {
      const r = this.dragRectTo(e.clientX, e.clientY);
      this.endDrag();
      if (!isClickSized(r)) {
        this.suppressClick = true;
        this.hovered = null;
        this.hooks.pickArea(r);
        return;
      }
    }
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
    let t = this.choose(this.target(e), e.clientX, e.clientY);
    if (t && (this.widen.active || e.altKey)) t = widenedTarget(t, this.widen.active ? this.widen.level : 1).target;
    if (t instanceof Element) this.hooks.pickElement(t);
    else if (t) this.hooks.pickRange(t);
  };

  private onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      if (this.drag) this.endDrag();
      else this.hooks.cancel();
      return;
    }
    if (this.key(e.key, true)) e.preventDefault();
  };

  private onKeyUp = (e: KeyboardEvent) => {
    this.key(e.key, false);
  };
}

/** `r` with the edges `outlineBox` reads. */
const rectOf0 = (r: AreaRect) => ({ ...r, right: r.left + r.width, bottom: r.top + r.height });

const sameTarget = (a: Element | Range | null, b: Element | Range | null) =>
  a === b || (a instanceof Range && b instanceof Range && sameRange(a, b));

const sameRange = (a: Range, b: Range) =>
  a.startContainer === b.startContainer && a.startOffset === b.startOffset && a.endContainer === b.endContainer && a.endOffset === b.endOffset;
