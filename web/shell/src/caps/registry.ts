// One handler factory per capability the shell serves.
import { artifactHandler } from "./artifact";
import { assetsHandler } from "./assets";
import { dbHandler } from "./db";
import { downloadsHandler } from "./downloads";
import type { HandlerFactory } from "./host";
import { permissionsHandler } from "./permissions";
import { userHandler } from "./user";

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: artifactHandler,
  assets: assetsHandler,
  db: dbHandler,
  downloads: downloadsHandler,
  permissions: permissionsHandler,
  user: userHandler,
};
