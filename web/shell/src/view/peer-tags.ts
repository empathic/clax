// Name tags for other people's pointers in the content frame (the live
// layers, spec 2026-10-11-clax-dev-hosted-design §16.3). The frame draws
// unnamed arrows and says where (`clax:peer-at`, its viewport's pixels,
// which are the stage's: the frame fills it); the names stay out of the
// frame and are drawn here over it, clipped to its box, below the stage's
// island (the pins and the composer stay over them). The tags are the
// arrows' colours (one set for both themes: they sit over the page, whose
// colours Clax does not set; white text on each is 5.4:1 or more). Part of
// the live view's chunk, after the first paint; its styles go in with the
// first tag.

import { type PeerTag, liveHex } from "./live-layers";

const CSS = `.peer-tags { position: absolute; inset: 0; overflow: hidden; pointer-events: none; z-index: 2; }
.peer-tag { position: absolute; left: 0; top: 0; max-width: 14em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; padding: 0 6px; border-radius: 3px 8px 8px 8px; font: 600 11px/16px var(--font); color: #fff; box-shadow: 0 1px 2px rgba(0, 0, 0, .25); transition: transform .12s linear; will-change: transform; }
@media (prefers-reduced-motion: reduce) { .peer-tag { transition: none; } }`;

/** The tags over one stage: `show` draws each set in place of the last. */
export class PeerTags {
  private box: HTMLElement | null = null;
  private readonly spans = new Map<string, HTMLElement>();

  constructor(private readonly stage: () => HTMLElement | null) {}

  show(tags: readonly PeerTag[]): void {
    if (!this.box) {
      const stage = tags.length ? this.stage() : null;
      if (!stage) return;
      const doc = stage.ownerDocument;
      if (!doc.getElementById("clax-peer-tags-css")) {
        const style = doc.createElement("style");
        style.id = "clax-peer-tags-css";
        style.textContent = CSS;
        doc.head.appendChild(style);
      }
      this.box = doc.createElement("div");
      this.box.className = "peer-tags";
      this.box.setAttribute("aria-hidden", "true");
      stage.insertBefore(this.box, stage.querySelector(":scope > .island"));
    }
    const keep = new Set<string>();
    for (const t of tags) {
      keep.add(t.key);
      let s = this.spans.get(t.key);
      if (!s) {
        s = this.box.ownerDocument.createElement("span");
        s.className = "peer-tag";
        this.spans.set(t.key, s);
        this.box.appendChild(s);
      }
      if (s.textContent !== t.name) s.textContent = t.name;
      s.style.background = liveHex(t.color);
      s.style.transform = `translate(${t.x + 12}px, ${t.y + 12}px)`;
    }
    for (const [k, s] of this.spans) if (!keep.has(k)) { s.remove(); this.spans.delete(k); }
  }

  /** Takes the tags off the stage. */
  remove(): void {
    this.box?.remove();
    this.box = null;
    this.spans.clear();
  }
}
