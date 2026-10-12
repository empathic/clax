// Whether the shell's page counts as shown. In a browser, the document's own
// visibility. In the Clax app the WebView may report the document hidden
// while the app shows it (macOS WKWebView does from load on) and then runs no
// animation frames; there the app's host says (`app/host.ts`,
// `claxHost.setVisible`), and replaces `hidden` and `shownByHost`.
export const visibility = {
  /** Whether the page counts as hidden. */
  hidden: (d: Document): boolean => d.visibilityState === "hidden",
  /** Whether the app's host says the page is shown (never in a browser):
   * a wait for a frame may then fall back to a timer, as the WebView may run
   * none however long the page is shown. */
  shownByHost: (): boolean => false,
};
