// One handler factory per capability the shell serves.
import { artifactHandler } from "./artifact";
import { assetsHandler } from "./assets";
import { commentsHandler } from "./comments";
import { dbHandler } from "./db";
import type { DeviceCap } from "./device-link";
import { downloadsHandler } from "./downloads";
import type { HandlerFactory } from "./host";
import { lazyHandler } from "./lazy";
import { permissionsHandler } from "./permissions";
import { userHandler } from "./user";

/** A device capability's handler, its code loaded on the page's first call. */
const device = (cap: DeviceCap) => lazyHandler(() => import("./device").then(m => m.deviceHandler(cap)));

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: artifactHandler,
  assets: assetsHandler,
  comments: commentsHandler,
  db: dbHandler,
  notify: device("notify"),
  share: device("share"),
  files: device("files"),
  camera: device("camera"),
  location: device("location"),
  downloads: downloadsHandler,
  net: lazyHandler(() => import("./net").then(m => m.netHandler)),
  permissions: permissionsHandler,
  room: lazyHandler(() => import("./room").then(m => m.roomHandler)),
  sample: lazyHandler(() => import("./sample").then(m => m.sampleHandler)),
  user: userHandler,
};
