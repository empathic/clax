// What claude.use() resolves: the capabilities' page-side members, and the
// client of the page's own origin that `db`, `assets` and `user` call.
// Loaded on the page's first claude.use().
export { resolveUse } from "../caps/resolve";
export type { CapsEnv } from "../caps/comments";

let routes: Routes | undefined;

/** The app shell's welcome: reports the page's scroll position and restores
 * `y` (`appScroll`), and watches the page's routes and its edge colours
 * (`reportChrome`), once per document. */
export function app(win: Window, post: (m: { type: "clax:scroll"; file: string; y: number } | { type: "clax:chrome"; top: string; bottom: string } | RouteReport) => void, file: string, y?: number): Routes {
  appScroll(win, post, file, y);
  if (!routes) reportChrome(win, post);
  return routes ??= appRoutes(win, post);
}
