// Agents' drafts in the artifact view (hosted spec
// 2026-10-11-clax-dev-hosted-design §16.4), loaded off the artifact entry
// once the daemon's listing or a `drafts` event names a draft: the drafts
// list the view keeps, the draft shown in the frame in place of the version
// (tinted, at the page the frame showed), the count of what its page tinted,
// and the banner that offers Show draft and Back.
import type { FrameHost } from "./frame-host";
import { startSrc } from "../origin";
import type { ViewState } from "./artifact-controller";
import { type DraftView, draftSrc, oldestFirst, parseDiff, tintOf } from "./draft-model";
import DraftBanner from "../ui/DraftBanner.svelte";

/** What the drafts need of the controller (`ArtifactController`'s own). */
export type DraftsHost = {
  readonly id: string;
  readonly startFile: string;
  readonly frame: FrameHost | null;
  readonly state: { get(): ViewState };
  set(patch: Partial<ViewState> | ((s: ViewState) => Partial<ViewState>)): void;
  /** Opens the frame on what the state shows: the shown draft, or the version. */
  showFrame(): void;
  /** The site origin the frame loads from (`ArtifactController.site`). */
  site(s: ViewState): string;
};

export class Drafts {
  /** The banner, for the stage island. */
  readonly Banner = DraftBanner;
  /** The page the frame showed when a draft was first shown: the draft is shown at it. */
  private page: string | null = null;
  private readonly onDiff: (e: MessageEvent) => void;

  constructor(private readonly host: DraftsHost) {
    // A draft page's count of what it tinted (the daemon's tint script),
    // heard only from the frame while it shows a draft.
    this.onDiff = e => {
      const changed = parseDiff(e.data);
      const el = host.frame?.el;
      if (changed && host.state.get().showDraft !== null && el && e.source === el.contentWindow) host.set({ draftChanged: changed });
    };
    addEventListener("message", this.onDiff);
  }

  dispose(): void { removeEventListener("message", this.onDiff); }

  /** The draft the frame shows, while it is live. */
  shown(s: ViewState = this.host.state.get()): DraftView | undefined {
    return s.showDraft === null ? undefined : s.drafts.find(d => d.agent === s.showDraft);
  }

  /** Opens `frame` on the shown draft, through its site's handshake, if one
   * is shown; whether it did. A draft page never greets: the gate stays
   * closed and no pins are drawn over it. */
  showIn(frame: FrameHost, s: ViewState): boolean {
    const d = this.shown(s);
    if (!d || s.origin === undefined) return false;
    const site = this.host.site(s);
    frame.show(startSrc(site, draftSrc(d, site, this.page ?? this.host.startFile, tint())), `draft-${d.agent}-${d.token}`);
    return true;
  }

  /** Shows `agent`'s draft in the frame in place of the version, at the
   * page the frame shows; comment mode ends (comments go on versions). */
  show(agent: string): void {
    const s = this.host.state.get();
    if (!s.drafts.some(d => d.agent === agent) || s.showDraft === agent) return;
    if (s.showDraft === null) this.page = s.file ?? this.host.startFile;
    this.host.set({ showDraft: agent, draftChanged: null, commenting: false });
    this.host.showFrame();
  }

  /** Shows the version again. */
  back(): void {
    if (this.host.state.get().showDraft === null) return;
    this.page = null;
    this.host.set({ showDraft: null, draftChanged: null });
    this.host.showFrame();
  }

  /** The live drafts are `list` now. A shown draft that was revised is
   * shown again at its new revision; one that ended gives way to the
   * version. */
  changed(list: DraftView[]): void {
    const before = this.shown();
    const drafts = oldestFirst(list);
    const kept = before !== undefined && drafts.some(d => d.agent === before.agent);
    this.host.set(s => ({ drafts, showDraft: kept ? s.showDraft : null }));
    const now = this.shown();
    if (before === undefined || now?.token === before.token) return;
    if (!now) this.page = null;
    this.host.set({ draftChanged: null });
    this.host.showFrame();
  }
}

/** The agents' colour for a draft's tint: the theme's `--agent`. */
function tint(): string {
  return tintOf(typeof getComputedStyle === "function" ? getComputedStyle(document.documentElement).getPropertyValue("--agent") : "");
}
