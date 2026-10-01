/** The sandbox a frame gets when artifacts have no origin of their own. */
export const FRAME_SANDBOX = "allow-scripts allow-forms allow-modals allow-popups allow-downloads";

/** The content `<iframe>`, first in the stage. The shell never re-creates it
 * on a render: it is replaced only when `key` (version and frame mode)
 * changes, and one the daemon already put in the stage is adopted when its
 * `src` and sandboxing are what the shell would have made. `onLoad` runs on
 * every load of a document in it, navigations inside the frame included. */
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
  show(src: string, sandboxed: boolean, key: string): HTMLIFrameElement {
    if (this.el && this.key === key) {
      if (src !== this.src) { this.src = src; this.el.src = src; }
      return this.el;
    }
    const served = this.el ? null : this.stage.querySelector<HTMLIFrameElement>(":scope > iframe.frame");
    if (served && served.getAttribute("src") === src && served.hasAttribute("sandbox") === sandboxed) {
      this.take(served, key, src);
      return served;
    }
    served?.remove();
    this.remove();
    const el = this.stage.ownerDocument.createElement("iframe");
    el.className = "frame";
    el.title = "artifact content";
    el.setAttribute("allow", "clipboard-write; fullscreen");
    if (sandboxed) el.setAttribute("sandbox", FRAME_SANDBOX);
    el.src = src;
    this.take(el, key, src);
    this.stage.prepend(el);
    return el;
  }

  remove(): void {
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
