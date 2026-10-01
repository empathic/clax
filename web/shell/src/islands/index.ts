// The islands of the artifact view. Each island is its own mount root over
// one ArtifactController.
import type { ArtifactController } from "../view/artifact-controller";
import { svelteIslands } from "./svelte";

/** Renders an island into `target`; the result unmounts it. */
export type MountIsland = (target: HTMLElement, ctl: ArtifactController) => () => void;
export type Islands = { topbar: MountIsland; stage: MountIsland; sidebar: MountIsland };

export const ISLANDS: Islands = svelteIslands;
