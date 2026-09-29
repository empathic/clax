// One handler factory per capability the shell serves.
import { artifactHandler } from "./artifact";
import { dbHandler } from "./db";
import { downloadsHandler } from "./downloads";
import type { HandlerFactory } from "./host";
import { permissionsHandler } from "./permissions";

export const REGISTRY: Record<string, HandlerFactory> = {
  artifact: artifactHandler,
  db: dbHandler,
  downloads: downloadsHandler,
  permissions: permissionsHandler,
};
