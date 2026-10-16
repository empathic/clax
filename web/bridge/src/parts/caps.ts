// The capabilities' page-side members: loaded on the first claude.use() the
// shell grants. Also the app shell's scroll report and restore, and its
// watch of the page's pushState routes, loaded on its welcome (`app: true`).
import { type RouteReport, type Routes, appRoutes } from "../routes";
import { appScroll } from "../scroll";

export { localsFor } from "../caps/index";
export type { CapsEnv } from "../caps/comments";

let routes: Routes | undefined;

/** The app shell's welcome: reports the page's scroll position and restores
 * `y` (`appScroll`), and watches the page's routes, once per document. */
export function app(win: Window, post: (m: { type: "clax:scroll"; file: string; y: number } | RouteReport) => void, file: string, y?: number): Routes {
  appScroll(win, post, file, y);
  return routes ??= appRoutes(win, post);
}
