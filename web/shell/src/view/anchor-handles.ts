/** Thread anchors go to the frame under opaque handles, new for every page
 * that greets, so a page never learns a thread's store ID; the frame's
 * results are mapped back through `thread`. */
export class AnchorHandles {
  private byHandle = new Map<string, string>();
  private byThread = new Map<string, string>();

  /** The thread's handle on this page, made on first use. */
  handle(threadId: string): string {
    let h = this.byThread.get(threadId);
    if (!h) {
      const b = new Uint8Array(12);
      crypto.getRandomValues(b);
      h = `a${Array.from(b, x => x.toString(16).padStart(2, "0")).join("")}`;
      this.byThread.set(threadId, h);
      this.byHandle.set(h, threadId);
    }
    return h;
  }

  /** The handle already given to the thread on this page, if any. */
  known(threadId: string): string | null {
    return this.byThread.get(threadId) ?? null;
  }

  /** The thread a handle given on this page stands for. */
  thread(handle: string): string | undefined {
    return this.byHandle.get(handle);
  }

  /** Another page greeted: every handle given so far stops mapping back. */
  forget(): void {
    this.byHandle = new Map();
    this.byThread = new Map();
  }
}
