/** The artifact view's static layout. `.island` elements are `display:
 * contents` mount points for the topbar controls, the stage's overlays and the
 * sidebar; `<!--clax:frame-->` marks where the daemon may put the content
 * frame. The daemon may send this markup in the page, and `skeleton` adopts it. */
export const SKELETON_HTML = `<header class="topbar"><a href="/" title="Gallery">←</a><h1>Clax</h1><div class="island"></div></header><div class="viewer"><div class="stage"><!--clax:frame--><div class="island"></div></div><div class="island"></div></div>`;

export type Skeleton = { page: HTMLElement; title: HTMLElement; viewer: HTMLElement; stage: HTMLElement; topbarIsland: HTMLElement; stageIsland: HTMLElement; sidebarIsland: HTMLElement };

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
  return { page: p, title: q(".topbar > h1"), viewer: q(".viewer"), stage: q(".stage"), topbarIsland: q(".topbar > .island"), stageIsland: q(".stage > .island"), sidebarIsland: q(".viewer > .island") };
}
