// Which framework mounts each island of the artifact view. Each island is its
// own mount root over one ArtifactController, so the page may mix frameworks
// while the shell is ported.
import type { ArtifactController } from "../view/artifact-controller";
import { preactIslands } from "./preact";

/** Renders an island into `target`; the result unmounts it. */
export type MountIsland = (target: HTMLElement, ctl: ArtifactController) => () => void;
export type Islands = { topbar: MountIsland; stage: MountIsland; sidebar: MountIsland };

export const ISLANDS: Islands = { topbar: preactIslands.topbar, stage: preactIslands.stage, sidebar: preactIslands.sidebar };
