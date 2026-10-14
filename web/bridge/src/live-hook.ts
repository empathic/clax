// The live layers' hand-off in comment mode's part (spec
// 2026-10-11-clax-dev-hosted-design §16.3). The drawing and the pointer
// reports are the `live` part (`live.ts`), loaded here on need: for the
// first open quote to highlight, or for the first `clax:peers` that draws
// someone or says someone else is on the artifact. A page nobody else is
// on reports no pointer and loads none of it. What arrives while the part
// loads is applied in order once it has.

import type { Anchor, BridgeToShell, LiveSelection, ShellToBridge } from "./protocol";
import type { AreaRect } from "./area";
import type { Resolved } from "./anchor";
import type { LivePart } from "./parts/types";
import type { Timers } from "./live";

export class LiveHook {
  private part: LivePart | null = null;
  private loading: Promise<LivePart> | null = null;
  private layer: InstanceType<LivePart["LiveLayer"]> | null = null;
  /** Someone else was here once: the layer is made (or being made). */
  private wanted = false;
  private dragRect: AreaRect | null = null;
  private picking: Element | Range | AreaRect | null = null;

  constructor(private readonly doc: Document, private readonly post: (m: BridgeToShell) => void, private readonly timers: Timers, private readonly load: () => Promise<LivePart>) {}

  /** Runs `f` with the live part once it has loaded; a failed load is
   * reported by the loader and tried again at the next need. */
  private need(f: (p: LivePart) => void): void {
    if (this.part) { f(this.part); return; }
    const p = this.loading ??= this.load().then(part => (this.part = part));
    p.then(f, () => { if (this.loading === p) this.loading = null; });
  }

  /** A `clax:peers`: the people to draw, and whether anyone else is here. */
  peers({ peers, here }: Extract<ShellToBridge, { type: "clax:peers" }>): void {
    if (!this.wanted && !(Array.isArray(peers) && peers.length) && here !== true) return;
    this.wanted = true;
    this.need(p => {
      if (!this.layer) {
        this.layer = new p.LiveLayer(this.doc, this.post, this.timers);
        this.layer.setDrag(this.dragRect);
        this.layer.setPick(this.pickOf(this.picking));
      }
      this.layer.peers(p.acceptPeers(peers));
    });
  }

  /** Highlights the quotes of `anchors` marked `highlight` (the open
   * threads'), as `cache` resolves them now. */
  quotes(anchors: readonly { id: string; anchor: Anchor; sameVersion?: boolean; highlight?: boolean }[], cache: { resolve(id: string, a: Anchor, same: boolean): Resolved | null }): void {
    const ranges = anchors.flatMap(a => {
      const r = a.highlight === true ? cache.resolve(a.id, a.anchor, a.sameVersion === true)?.range : null;
      return r ? [r] : [];
    });
    if (ranges.length || this.part || this.loading) this.need(p => p.highlightQuotes(this.doc, ranges));
  }

  /** The area being drawn in comment mode, or null when none is. */
  drag(r: AreaRect | null): void {
    this.dragRect = r;
    this.layer?.setDrag(r);
  }

  /** Stops the layer's reports and removes its drawing. */
  destroy(): void {
    this.layer?.destroy();
    this.layer = null;
    this.wanted = false;
  }

  /** A pick of `t` in comment mode: others see it while its composer is open. */
  picked(t: Element | Range | AreaRect): void {
    this.picking = t;
    this.layer?.setPick(this.pickOf(t));
  }

  /** `t` as a live selection; null without the live part. */
  private pickOf(t: Element | Range | AreaRect | null): LiveSelection | null {
    const p = this.part;
    if (!p || !t) return null;
    // By shape, not by the page's own `Element` and `Range`, which it may replace.
    return "nodeType" in t ? p.elementSelection(this.doc, t) : "collapsed" in t ? p.rangeSelection(this.doc, t) : p.areaSelection(this.doc, t);
  }
}
