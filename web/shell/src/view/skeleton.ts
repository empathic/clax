import { MARK_SVG } from "./mark";

/** The artifact view's static layout (spec §8, "Top bar"). `.island`
 * elements are `display: contents` mount points for the top bar's controls,
 * the stage's overlays and the sidebar; `<!--clax:frame-->` marks where the
 * daemon may put the content frame; `<h1>Clax</h1>` is where it writes the
 * title. The daemon may send this markup in the page, and `skeleton` adopts it. */
export const SKELETON_HTML = `<header class="topbar"><a href="/" class="home" aria-label="Gallery">${MARK_SVG}</a><div class="ttl"><h1>Clax</h1><span class="by"></span></div><div class="island"></div></header><div class="viewer"><div class="stage"><!--clax:frame--><div class="island"></div></div><div class="island"></div></div>`;

export type Skeleton = { page: HTMLElement; topbar: HTMLElement; title: HTMLElement; by: HTMLElement; viewer: HTMLElement; stage: HTMLElement; topbarIsland: HTMLElement; stageIsland: HTMLElement; sidebarIsland: HTMLElement };

/** The skeleton in `root`, adopted when present (sent by the daemon), else created. */
export function skeleton(root: HTMLElement): Skeleton {
  let page = root.querySelector<HTMLElement>(":scope > .page");
  if (!page) {
    page = root.ownerDocument.createElement("div");
    page.className = "page";
    page.innerHTML = SKELETON_HTML;
    root.append(page);
  }
  const p: HTMLElement = page;
  const q = (sel: string) => p.querySelector<HTMLElement>(sel)!;
  return { page: p, topbar: q(".topbar"), title: q(".topbar h1"), by: q(".topbar .by"), viewer: q(".viewer"), stage: q(".stage"), topbarIsland: q(".topbar > .island"), stageIsland: q(".stage > .island"), sidebarIsland: q(".viewer > .island") };
}
