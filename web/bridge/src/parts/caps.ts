// The capabilities' page-side members: loaded on the first claude.use() the
// shell grants. Also the app shell's scroll report and restore, and its
// watch of the page's pushState routes, loaded on its welcome (`app: true`).
export { localsFor } from "../caps/index";
export type { CapsEnv } from "../caps/comments";
export { appScroll } from "../scroll";
export { appRoutes } from "../routes";
