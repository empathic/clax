import { MARK_SVG } from "./mark";

/** The artifact view's static layout (spec §8, "Top bar"), as the first
 * frame paints it, before any script: the mounted view's geometry, as
 * `web/shell/artifact.html` carries it (`skeleton.test.ts` holds them
 * equal), and as a test puts it in the root it mounts the view in. Apart
 * from `skeleton.ts`, so the artifact entry carries it once, in its HTML.
 * `.island` elements are `display: contents` mount points for the top
 * bar's controls, the stage's overlays and the sidebar; `<!--clax:frame-->`
 * marks where the daemon may put the content frame; `<h1></h1>` is where it
 * writes the title (empty, a placeholder bar, until the title is known; the
 * line under it held while `.ttl` is `pending`). The stage says it is
 * loading (`.stage-wait`, shown after a moment) until the frame shows its
 * page or the stage a message; the drawer's box (`.drawer.held`) holds the
 * threads' place where they open docked, until the sidebar island draws
 * its own, and the closed drawer's strip (`.rail-held`) where it shows
 * (theme.css). */
export const SKELETON_HTML = `<header class="topbar"><a href="/" class="home" aria-label="Gallery">${MARK_SVG}</a><div class="ttl pending"><h1></h1><span class="by"></span></div><div class="island"></div></header><div class="viewer"><div class="stage"><!--clax:frame--><p class="stage-wait" role="status">Loading…</p><div class="island"></div></div><aside class="sidebar drawer held" aria-label="Comment threads" aria-busy="true"></aside><aside class="rail-held" aria-hidden="true"></aside><div class="island"></div></div>`;
