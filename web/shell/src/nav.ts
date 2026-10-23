// Whole-document navigations of the shell, behind one object so tests can
// observe them (jsdom does not navigate).
export const nav = {
  /** Loads `url` in the shell window. */
  assign(url: string): void {
    location.assign(url);
  },
  /** Loads `url` in the shell window, also when it differs from the
   * current address only in its fragment (which `assign` would not load). */
  reload(url: string): void {
    history.replaceState(history.state, "", url);
    location.reload();
  },
};
