// The artifact view's islands in Svelte. Each mounts once over the controller
// and renders from its store; Svelte batches the store's changes into one
// update per microtask.
import { type Component, mount, unmount } from "svelte";
import SidebarIsland from "../ui/SidebarIsland.svelte";
import StageIsland from "../ui/StageIsland.svelte";
import TopbarIsland from "../ui/TopbarIsland.svelte";
import type { ArtifactController } from "../view/artifact-controller";
import type { Islands, MountIsland } from "./index";

const island = (C: Component<{ ctl: ArtifactController }>): MountIsland => (target, ctl) => {
  const c = mount(C, { target, props: { ctl } });
  return () => { void unmount(c); };
};

export const svelteIslands: Islands = { topbar: island(TopbarIsland), stage: island(StageIsland), sidebar: island(SidebarIsland) };
