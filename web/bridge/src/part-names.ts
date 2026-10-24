// Every lazy part's name, as Parts (parts/types.ts) lists them: the bridge has a
// loader for each, and the bridge build (vite.bridge.config.ts) fails when the
// parts build made no entry for one. A part missing here, or one Parts does
// not have, is a type error.
import type { Parts } from "./parts/types";

const named = { comment: 0, clip: 0, caps: 0, origin: 0, room: 0, sample: 0, live: 0, test: 0 } satisfies Record<keyof Parts, 0>;

export const PART_NAMES = Object.keys(named) as (keyof Parts)[];
