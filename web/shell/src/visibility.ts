// Whether the shell's page counts as shown. In a browser, the document's own
// visibility. In the Clax app the WebView may report the document hidden
// while the app shows it (macOS WKWebView does from load on) and then runs no
// animation frames; there the app's host says (`app/host.ts`,
// `claxHost.setVisible`): it replaces `hidden` and `shown`, and calls
// `changed` whenever its word changes.
const listeners = new Set<() => void>();

export const visibility = {
  /** Whether the page counts as hidden. */
  hidden: (d: Document): boolean => d.visibilityState === "hidden",
  /** What the app's host says: true for shown, false for hidden, null for
   * no word (a browser, or the app before its `init`). While the host says
   * shown, a wait for a frame may fall back to a timer, as the WebView may
   * run none however long the page is shown. */
  shown: (): boolean | null => null,
  /** Tells every `onHostVisibility` listener the host's word changed. */
  changed(): void { for (const f of [...listeners]) f(); },
};

/** Calls `f` whenever the app's host's word on visibility changes, until the
 * returned function runs. */
export function onHostVisibility(f: () => void): () => void {
  listeners.add(f);
  return () => { listeners.delete(f); };
}
