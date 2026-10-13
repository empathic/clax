// One handler factory per capability the shell serves. Those a page calls
// rarely, or only on a viewer's action (assets, downloads, room, sample), are
// lazy chunks loaded on the page's first call (lazy.ts), off the artifact entry.
import { artifactHandler } from "./artifact";
import { commentsHandler } from "./comments";
import { dbHandler } from "./db";
import { deviceHandler } from "./device";
import type { HandlerFactory } from "./host";
import { lazyHandler } from "./lazy";
import { permissionsHandler } from "./permissions";
import { userHandler } from "./user";

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: artifactHandler,
  assets: lazyHandler(() => import("./assets").then(m => m.assetsHandler)),
  comments: commentsHandler,
  db: dbHandler,
  notify: deviceHandler("notify"),
  share: deviceHandler("share"),
  files: deviceHandler("files"),
  camera: deviceHandler("camera"),
  location: deviceHandler("location"),
  downloads: lazyHandler(() => import("./downloads").then(m => m.downloadsHandler)),
  net: lazyHandler(() => import("./net").then(m => m.netHandler)),
  permissions: permissionsHandler,
  room: lazyHandler(() => import("./room").then(m => m.roomHandler)),
  sample: lazyHandler(() => import("./sample").then(m => m.sampleHandler)),
  user: userHandler,
};
