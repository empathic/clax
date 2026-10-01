// The islands of the artifact view, in Svelte. Each island is its own mount
// root over one ArtifactController: it mounts once and renders from the
// controller's store; Svelte batches the store's changes into one update per
// microtask.
import { type Component, mount, unmount } from "svelte";
import SidebarIsland from "../ui/SidebarIsland.svelte";
import StageIsland from "../ui/StageIsland.svelte";
import TopbarIsland from "../ui/TopbarIsland.svelte";
import type { ArtifactController } from "../view/artifact-controller";

/** Renders an island into `target`; the result unmounts it. */
export type MountIsland = (target: HTMLElement, ctl: ArtifactController) => () => void;

const island = (C: Component<{ ctl: ArtifactController }>): MountIsland => (target, ctl) => {
  const c = mount(C, { target, props: { ctl } });
  return () => { void unmount(c); };
};

export const islands: { topbar: MountIsland; stage: MountIsland; sidebar: MountIsland } = {
  topbar: island(TopbarIsland),
  stage: island(StageIsland),
  sidebar: island(SidebarIsland),
};
