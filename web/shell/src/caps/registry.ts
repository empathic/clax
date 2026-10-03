// One handler factory per capability the shell serves.
import { artifactHandler } from "./artifact";
import { assetsHandler } from "./assets";
import { commentsHandler } from "./comments";
import { dbHandler } from "./db";
import { downloadsHandler } from "./downloads";
import type { HandlerFactory } from "./host";
import { lazyHandler } from "./lazy";
import { permissionsHandler } from "./permissions";
import { userHandler } from "./user";

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: artifactHandler,
  assets: assetsHandler,
  comments: commentsHandler,
  db: dbHandler,
  downloads: downloadsHandler,
  permissions: permissionsHandler,
  room: lazyHandler(() => import("./room").then(m => m.roomHandler)),
  sample: lazyHandler(() => import("./sample").then(m => m.sampleHandler)),
  user: userHandler,
};
