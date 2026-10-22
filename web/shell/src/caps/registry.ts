// One handler factory per capability the shell serves a framed page. `db`,
// `assets`, `user` and `room` are the page's own origin's (the bridge calls
// its site, web/bridge/src/caps/origin.ts): the shell keeps only `db`'s
// change reports and `user`'s lookups of other people. Those a page calls
// rarely, or only on a viewer's action (downloads, sample, the lookups), are
// lazy chunks loaded on the page's first call (lazy.ts), off the artifact entry.
import { artifactHandler } from "./artifact";
import { commentsHandler } from "./comments";
import { dbHandler } from "./db";
import type { LinkedCap } from "./device-link";
import { type ArrivalGesture, frameGesture, frameGestureStrict } from "./gesture";
import type { HandlerFactory } from "./host";
import { lazyHandler } from "./lazy";
import { permissionsHandler } from "./permissions";

/** The composer tier's gesture at a call's arrival (device, home). */
const composerAtArrival = (): ArrivalGesture => ({ composer: frameGesture() });

/** The strict tier's gesture at a call's arrival (artifact.publish). */
function strictAtArrival(): ArrivalGesture {
  const strict = frameGestureStrict();
  return { composer: strict !== "no_gesture", strict };
}

/** A device capability's handler (or `home`'s), its code loaded on the page's first call. */
const device = (cap: LinkedCap) => lazyHandler(() => import("./device").then(m => m.deviceHandler(cap)), composerAtArrival);

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: artifactHandler,
  comments: commentsHandler,
  db: dbHandler,
  notify: device("notify"),
  share: device("share"),
  file: device("file"),
  camera: device("camera"),
  location: device("location"),
  home: device("home"),
  downloads: lazyHandler(() => import("./downloads").then(m => m.downloadsHandler)),
  net: lazyHandler(() => import("./net").then(m => m.netHandler)),
  permissions: permissionsHandler,
  sample: lazyHandler(() => import("./sample").then(m => m.sampleHandler)),
  user: lazyHandler(() => import("./user").then(m => m.userHandler)),
};
