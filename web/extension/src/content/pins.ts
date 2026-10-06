// The threads' pins over the page (spec 2026-10-05 O4, L7): numbered, in the
// overlay's closed shadow root, carrying no text but their number. A press
// on a pin selects its thread in the side panel; only the viewer's own
// (trusted) presses count, and a pin never takes focus from the page.
import type { Placed } from "./resolver";

export const PIN_CSS = `.pin{position:fixed;z-index:2147483647;min-width:20px;height:20px;margin:-10px 0 0 -10px;padding:0 5px;border-radius:10px 10px 10px 2px;border:1.5px solid #fff;background:#ed5439;color:#2f0b04;font:600 11px/17px ui-sans-serif,-apple-system,system-ui,sans-serif;box-shadow:0 1px 3px rgba(47,11,4,.45);cursor:pointer;box-sizing:border-box;pointer-events:auto}
.pin.sel{box-shadow:0 0 0 3px #2f6f2a}`;

export class Pins {
  private els = new Map<string, HTMLButtonElement>();
  constructor(private readonly root: ShadowRoot, private readonly onPick: (threadId: string) => void) {}

  /** One pin per found thread at its box's top right corner, numbered;
   * a detached thread has none. */
  draw(placed: Placed[], selected: string | null): void {
    const live = new Set<string>();
    for (const p of placed) {
      if (!p.box || p.n === null) continue;
      live.add(p.id);
      let el = this.els.get(p.id);
      if (!el) {
        el = this.root.ownerDocument.createElement("button");
        el.type = "button";
        el.className = "pin";
        el.tabIndex = -1;
        const id = p.id;
        el.addEventListener("mousedown", e => { if (e.isTrusted) e.preventDefault(); });
        el.addEventListener("click", e => {
          if (!e.isTrusted) return;
          e.preventDefault();
          e.stopPropagation();
          this.onPick(id);
        });
        this.root.appendChild(el);
        this.els.set(p.id, el);
      }
      el.textContent = String(p.n);
      el.setAttribute("aria-label", `Thread ${p.n}`);
      el.classList.toggle("sel", p.id === selected);
      el.style.left = `${Math.round(p.box.x + p.box.w)}px`;
      el.style.top = `${Math.round(p.box.y)}px`;
    }
    for (const [id, el] of this.els) if (!live.has(id)) { el.remove(); this.els.delete(id); }
  }
}
