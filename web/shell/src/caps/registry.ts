// One handler factory per capability the shell serves.
import type { HandlerFactory } from "./host";
import { permissionsHandler } from "./permissions";

export const REGISTRY: Record<string, HandlerFactory> = {
  permissions: permissionsHandler,
};
