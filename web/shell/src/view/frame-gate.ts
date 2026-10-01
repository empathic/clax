/** Whether the frame's latest hello named the shown artifact and version: only
 * then are its capability requests answered and events pushed to it, so a
 * document the frame navigated to gets nothing. A frame load with no
 * matching hello since the previous load closes the gate as well, and a
 * later hello reopens it: a wrapped page's hello may arrive before or after
 * its load event (after it for some sandboxed loads), so this never shuts
 * out a wrapped page, and it shuts out a document without the bridge
 * whenever the page before it greeted before its own load.
 *
 * The caller judges a hello (`hello(ok)`): it matches when it names the shown
 * artifact and version and a page that version holds. */
export class FrameGate {
  open = false;
  private helloSinceLoad = false;

  /** Another artifact, version or origin is shown: closed until it greets.
   * Called as soon as they change, before the frame for them is inserted,
   * since that frame's hello can arrive before anything after that runs. */
  reset(): void {
    this.open = false;
    this.helloSinceLoad = false;
  }

  hello(ok: boolean): void {
    this.open = ok;
    if (ok) this.helloSinceLoad = true;
  }

  /** The frame loaded a document. True when no matching hello came since the
   * previous load: the gate closed, and the page and its pins are forgotten. */
  load(): boolean {
    const stale = !this.helloSinceLoad;
    if (stale) this.open = false;
    this.helloSinceLoad = false;
    return stale;
  }

  /** The frame's document said it is going away (`clax:bye`): closed until
   * a document greets, and its hello no longer counts for the next load, so
   * a document after it that loads without greeting closes the gate too. */
  bye(): void {
    this.open = false;
    this.helloSinceLoad = false;
  }

  /** The shell sent the frame to another page: closed until that page greets. */
  close(): void {
    this.open = false;
  }
}
