// One handler factory per capability the shell serves. Those a page calls
// rarely, or only on a viewer's action (artifact's publish, assets,
// downloads, room, sample, the device capabilities), are lazy chunks loaded
// on the page's first call (lazy.ts), off the artifact entry.
import { commentsHandler } from "./comments";
import { dbHandler } from "./db";
import type { LinkedCap } from "./device-link";
import type { HandlerFactory } from "./host";
import { lazyHandler } from "./lazy";
import { permissionsHandler } from "./permissions";
import { userHandler } from "./user";

/** A device capability's handler (or `home`'s), its code loaded on the page's first call. */
const device = (cap: LinkedCap) => lazyHandler(() => import("./device").then(m => m.deviceHandler(cap)));

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: lazyHandler(() => import("./artifact").then(m => m.artifactHandler)),
  assets: lazyHandler(() => import("./assets").then(m => m.assetsHandler)),
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
  user: userHandler,
};
