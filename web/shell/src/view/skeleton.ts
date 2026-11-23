/** The parts of the artifact view's static layout (`skeleton-html.ts`),
 * which the page carries (`web/shell/artifact.html`). */
export type Skeleton = { page: HTMLElement; topbar: HTMLElement; title: HTMLElement; by: HTMLElement; viewer: HTMLElement; stage: HTMLElement; topbarIsland: HTMLElement; stageIsland: HTMLElement; sidebarIsland: HTMLElement };

/** The skeleton the page carries in `root` (`.page`). */
export function skeleton(root: HTMLElement): Skeleton {
  const p = root.querySelector<HTMLElement>(":scope > .page");
  if (!p) throw new Error("the artifact page carries no skeleton (.page)");
  const q = (sel: string) => p.querySelector<HTMLElement>(sel)!;
  return { page: p, topbar: q(".topbar"), title: q(".topbar h1"), by: q(".topbar .by"), viewer: q(".viewer"), stage: q(".stage"), topbarIsland: q(".topbar > .island"), stageIsland: q(".stage > .island"), sidebarIsland: q(".viewer > .island") };
}
