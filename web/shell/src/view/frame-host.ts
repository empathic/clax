import { INSPECT_FRAME, MIGRATE_FRAME } from "../../../bridge/src/protocol";
import { inspectQuery, migrateQuery } from "../route";

/** The permissions every content frame delegates: storage access lets a
 * page on its site ask for its cookie where the browser keeps a frame's
 * cookies apart. */
const FRAME_ALLOW = "clipboard-write; fullscreen; storage-access";
const FRAME_TITLE = "artifact content";
/** The app's inspect view (`?app=1&inspect=1`, `app/host.ts`) names its
 * frame `INSPECT_FRAME` before it loads, so the page's bridge keeps its
 * errors from its first script (`bridge/src/inspect.ts`). */
const inspecting = () => inspectQuery(location.search);
/** The name the content frame gets: `MIGRATE_FRAME` in the app's migration
 * view (`?app=1&inspect=1&migrate=1`), whose page's bridge then runs none of
 * the page's own markup; `INSPECT_FRAME` in the inspect view; none elsewhere. */
const frameName = (): string | null => (migrateQuery(location.search) ? MIGRATE_FRAME : inspecting() ? INSPECT_FRAME : null);

/** Whether `el`, a frame the daemon put in the stage, is the one the shell
 * would make for `src`: the same `src`, `allow`, title and class, no
 * sandbox, and named as `frameName` says (the daemon names it so too,
 * `frame_name` in `crates/clax-server/src/shell/boot.rs`). */
function servedAs(el: HTMLIFrameElement, src: string): boolean {
  return el.getAttribute("name") === frameName() && el.getAttribute("src") === src
    && !el.hasAttribute("sandbox")
    && el.getAttribute("allow") === FRAME_ALLOW
    && el.getAttribute("title") === FRAME_TITLE
    && el.className === "frame";
}

/** The content `<iframe>`, first in the stage. The shell never re-creates it
 * on a render: it is replaced only when `key` (version, or draft) changes,
 * and one the daemon already put in the stage is adopted when its `src` and
 * attributes are what the shell would have made. Any other frame in the
 * stage is removed. Adopting a frame opens nothing: the gate opens only on
 * a hello.
 * `onLoad` runs on every load of a document in it, navigations inside the
 * frame included (loads before the adoption are the caller's to replay). */
export class FrameHost {
  el: HTMLIFrameElement | null = null;
  private key = "";
  /** The `src` the frame was last given (not where it has navigated since). */
  private src = "";
  private readonly loaded = () => this.onLoad();

  constructor(private readonly stage: HTMLElement, private readonly onLoad: () => void) {}

  /** The frame for `key`, opened on `src` when it is made. A frame kept for
   * the same key is sent to `src` only when `src` differs from the one it was
   * last given, as a keyed element whose `src` attribute changed would be;
   * the same `src` leaves it where it has navigated to. */
  show(src: string, key: string): HTMLIFrameElement {
    if (this.el && this.key === key) {
      if (src !== this.src) { this.src = src; this.el.src = src; }
      return this.el;
    }
    const served = this.el ? null : this.stage.querySelector<HTMLIFrameElement>(":scope > iframe.frame");
    if (served && servedAs(served, src)) {
      this.take(served, key, src);
      return served;
    }
    served?.remove();
    this.remove();
    const el = this.stage.ownerDocument.createElement("iframe");
    el.className = "frame";
    const name = frameName();
    if (name) el.name = name;
    el.title = FRAME_TITLE;
    el.setAttribute("allow", FRAME_ALLOW);
    el.src = src;
    this.take(el, key, src);
    this.stage.prepend(el);
    return el;
  }

  /** Sends the frame to `url` (the handshake's next step) in place of its
   * history entry, so Back never returns to a step of the handshake,
   * keeping the `src` it was given. */
  go(url: string): void {
    const el = this.el;
    if (!el) return;
    try { el.contentWindow!.location.replace(url); } catch { el.src = url; }
  }

  /** Sends the frame back to the `src` it was last given, in place. */
  again(): void {
    this.go(this.src);
  }

  /** Removes the frame, and a served one not adopted. */
  remove(): void {
    if (!this.el) this.stage.querySelector(":scope > iframe.frame")?.remove();
    this.el?.removeEventListener("load", this.loaded);
    this.el?.remove();
    this.el = null;
    this.key = "";
    this.src = "";
  }

  private take(el: HTMLIFrameElement, key: string, src: string): void {
    el.addEventListener("load", this.loaded);
    this.el = el;
    this.key = key;
    this.src = src;
  }
}
