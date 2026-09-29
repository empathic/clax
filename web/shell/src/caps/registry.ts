// One handler factory per capability the shell serves.
import { dbHandler } from "./db";
import type { HandlerFactory } from "./host";
import { permissionsHandler } from "./permissions";

export const REGISTRY: Record<string, HandlerFactory> = {
  db: dbHandler,
  permissions: permissionsHandler,
};
