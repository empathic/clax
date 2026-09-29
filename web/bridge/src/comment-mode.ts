// Comment mode inside the page: an outline on the hovered element, a pin that
// follows the pointer, a click to pick an element, a text selection to pick a
// range, Escape to cancel. The overlay lives in a shadow root on <html>, so it
// never changes the page's body, selectors, or text.

import { OVERLAY_TAG } from "./anchor";

export interface ModeHooks {
  hover(el: Element | null): void;
  pickElement(el: Element): void;
  pickRange(r: Range): void;
  cancel(): void;
}

const CSS = `:host{all:initial}
.o{position:fixed;pointer-events:none;border:2px solid #c2410c;border-radius:3px;background:rgba(194,65,12,.08);z-index:2147483647;display:none}
.o.flash{animation:f .9s ease-out 2}
.pin{position:fixed;pointer-events:none;width:18px;height:18px;margin:-20px 0 0 4px;border-radius:50% 50% 50% 0;background:#c2410c;box-shadow:0 1px 4px rgba(0,0,0,.3);z-index:2147483647;display:none}
@keyframes f{50%{background:rgba(194,65,12,.35)}}`;

export class CommentMode {
  private on = false;
  private readonly host: HTMLElement;
  private readonly outline: HTMLElement;
  private readonly pin: HTMLElement;
  private suppressClick = false;
  private frame = 0;
  private hovered: Element | null = null;

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
      ["mouseup", this.onUp as EventListener],
      ["click", this.onClick as EventListener],
      ["keydown", this.onKey as EventListener],
    ];
    for (const [type, fn] of listeners) {
      if (on) this.doc.addEventListener(type, fn, true);
      else this.doc.removeEventListener(type, fn, true);
    }
    this.doc.documentElement.style.cursor = on ? "crosshair" : "";
    if (!on) {
      this.outline.style.display = "none";
      this.pin.style.display = "none";
      this.hovered = null;
    }
  }

  /** Outlines `target` briefly (after a scroll-to). */
  flash(target: Element | Range): void {
    this.place(target.getBoundingClientRect());
    this.outline.classList.add("flash");
    setTimeout(() => {
      this.outline.classList.remove("flash");
      if (!this.on) this.outline.style.display = "none";
    }, 1800);
  }

  private place(r: DOMRect): void {
    Object.assign(this.outline.style, { display: "block", left: `${r.left - 2}px`, top: `${r.top - 2}px`, width: `${r.width + 4}px`, height: `${r.height + 4}px` });
  }

  private target(e: Event): Element | null {
    const el = e.target as Element | null;
    if (!el || el === this.host || el === this.doc.documentElement || el === this.doc.body) return null;
    return el.closest?.(OVERLAY_TAG) ? null : el;
  }

  private onMove = (e: MouseEvent) => {
    Object.assign(this.pin.style, { display: "block", left: `${e.clientX}px`, top: `${e.clientY}px` });
    const t = this.target(e);
    if (t === this.hovered || this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.hovered = t;
      if (t) this.place(t.getBoundingClientRect());
      else this.outline.style.display = "none";
      this.hooks.hover(t);
    });
  };

  private onUp = () => {
    const sel = this.doc.getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount) return;
    const r = sel.getRangeAt(0).cloneRange();
    if (!this.doc.body.contains(r.commonAncestorContainer)) return;
    this.suppressClick = true;
    sel.removeAllRanges();
    this.hooks.pickRange(r);
  };

  private onClick = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (this.suppressClick) { this.suppressClick = false; return; }
    const t = this.target(e);
    if (t) this.hooks.pickElement(t);
  };

  private onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") this.hooks.cancel();
  };
}
