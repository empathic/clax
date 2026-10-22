// What claude.use() resolves of the framing shell's capabilities, and their
// page-side members: loaded on a framed page's first claude.use() of one.
// (The page's own origin's are parts/origin.ts.) Also the app shell's scroll
// report and restore, its watch of the page's pushState routes and its
// report of the page's edge colours, loaded on its welcome (`app: true`).
import { reportChrome } from "../chrome";
import { type RouteReport, type Routes, appRoutes } from "../routes";
import { appScroll } from "../scroll";

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
