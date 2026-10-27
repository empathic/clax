// One handler factory per capability the shell serves a framed page. `db`,
// `assets`, `user` and `room` are the page's own origin's (the bridge calls
// its site, web/bridge/src/caps/origin.ts): the shell keeps only `db`'s
// change reports, `room`'s viewer reports and `user`'s lookups of other
// people. Those a page calls rarely, or only on a viewer's action
// (artifact's publish, downloads, sample, the room's and the lookups, the
// device capabilities), are lazy chunks loaded on the page's first call
// (lazy.ts), off the artifact entry; those that act only on the viewer's
// gesture judge it when the call arrives.
import { commentsHandler } from "./comments";
import { dbHandler } from "./db";
import type { LinkedCap } from "./device-link";
import { type ArrivalGesture, frameGesture, frameGestureStrict } from "./gesture";
import type { HandlerFactory } from "./host";
import { lazyHandler } from "./lazy";
import { permissionsHandler } from "./permissions";

/** The composer tier's gesture at a call's arrival (device, home, shell). */
const composerAtArrival = (): ArrivalGesture => ({ composer: frameGesture() });

/** The strict tier's gesture at a call's arrival (artifact.publish). */
function strictAtArrival(): ArrivalGesture {
  const strict = frameGestureStrict();
  return { composer: strict !== "no_gesture", strict };
}

/** A device capability's handler (or `home`'s), its code loaded on the page's first call. */
const device = (cap: LinkedCap) => lazyHandler(() => import("./device").then(m => m.deviceHandler(cap)), composerAtArrival);

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: lazyHandler(() => import("./artifact").then(m => m.artifactHandler), strictAtArrival),
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
  room: lazyHandler(() => import("./room").then(m => m.roomHandler)),
  sample: lazyHandler(() => import("./sample").then(m => m.sampleHandler)),
  // `commentMode` is the composer tier's; `highlight` and `can` need no gesture.
  shell: lazyHandler(() => import("./shell").then(m => m.shellHandler), composerAtArrival),
  // In the Clax app, a page's calls to its own site's API (site.ts).
  site: lazyHandler(() => import("./site").then(m => m.siteHandler)),
  user: lazyHandler(() => import("./user").then(m => m.userHandler)),
};
