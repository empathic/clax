/** The sandbox a frame gets when artifacts have no origin of their own. */
export const FRAME_SANDBOX = "allow-scripts allow-forms allow-modals allow-popups allow-downloads";
/** The permissions every content frame delegates. */
const FRAME_ALLOW = "clipboard-write; fullscreen";
const FRAME_TITLE = "artifact content";

/** Whether `el`, a frame the daemon put in the stage, is the one the shell
 * would make for `src` (fragment aside: the daemon never sees it): the same
 * `src`, `allow`, title and class, and the same sandbox, exactly, or none. */
function servedAs(el: HTMLIFrameElement, src: string, sandboxed: boolean): boolean {
  const hash = src.indexOf("#");
  return el.getAttribute("src") === (hash < 0 ? src : src.slice(0, hash))
    && el.getAttribute("sandbox") === (sandboxed ? FRAME_SANDBOX : null)
    && el.getAttribute("allow") === FRAME_ALLOW
    && el.getAttribute("title") === FRAME_TITLE
    && el.className === "frame";
}

/** The content `<iframe>`, first in the stage. The shell never re-creates it
 * on a render: it is replaced only when `key` (version and frame mode)
 * changes, and one the daemon already put in the stage is adopted when its
 * `src` (without the fragment) and attributes are what the shell would have
 * made. Any other frame in the stage is removed. Adopting a frame opens nothing: the gate opens only on a hello.
 * An adopted frame is not sent to `src`'s fragment (the daemon's `src` has
 * none): setting it on a frame whose document has loaded is a fragment
 * navigation that, in Chromium, fires a load event at the frame element
 * with no new document. The fragment is kept for the caller instead
 * (`takeFragment`), to hand to the page once it greets.
 * `onLoad` runs on every load of a document in it, navigations inside the
 * frame included (loads before the adoption are the caller's to replay). */
export class FrameHost {
  el: HTMLIFrameElement | null = null;
  private key = "";
  /** The `src` the frame was last given (not where it has navigated since). */
  private src = "";
  private readonly loaded = () => this.onLoad();
  /** The fragment of the `src` an adopted frame was not sent to. */
  private fragment: string | null = null;

  constructor(private readonly stage: HTMLElement, private readonly onLoad: () => void) {}

  /** The frame for `key`, opened on `src` when it is made. A frame kept for
   * the same key is sent to `src` only when `src` differs from the one it was
   * last given, as a keyed element whose `src` attribute changed would be;
   * the same `src` leaves it where it has navigated to. */
  show(src: string, sandboxed: boolean, key: string): HTMLIFrameElement {
    if (this.el && this.key === key) {
      if (src !== this.src) { this.src = src; this.el.src = src; }
      return this.el;
    }
    const served = this.el ? null : this.stage.querySelector<HTMLIFrameElement>(":scope > iframe.frame");
    if (served && servedAs(served, src, sandboxed)) {
      this.take(served, key, src);
      const hash = src.indexOf("#");
      if (hash >= 0) this.fragment = src.slice(hash);
      return served;
    }
    served?.remove();
    this.remove();
    const el = this.stage.ownerDocument.createElement("iframe");
    el.className = "frame";
    el.title = FRAME_TITLE;
    el.setAttribute("allow", FRAME_ALLOW);
    if (sandboxed) el.setAttribute("sandbox", FRAME_SANDBOX);
    el.src = src;
    this.take(el, key, src);
    this.stage.prepend(el);
    return el;
  }

  /** The fragment an adopted frame was not sent to, once; null when none. */
  takeFragment(): string | null {
    const f = this.fragment;
    this.fragment = null;
    return f;
  }

  /** Removes the frame, and a served one not adopted. */
  remove(): void {
    if (!this.el) this.stage.querySelector(":scope > iframe.frame")?.remove();
    this.el?.removeEventListener("load", this.loaded);
    this.el?.remove();
    this.el = null;
    this.key = "";
    this.src = "";
    this.fragment = null;
  }

  private take(el: HTMLIFrameElement, key: string, src: string): void {
    el.addEventListener("load", this.loaded);
    this.el = el;
    this.key = key;
    this.src = src;
  }
}
