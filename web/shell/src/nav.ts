// Whole-document navigations of the shell, behind one object so tests can
// observe them (jsdom does not navigate).
export const nav = {
  /** Loads `url` in the shell window. */
  assign(url: string): void {
    location.assign(url);
  },
};
