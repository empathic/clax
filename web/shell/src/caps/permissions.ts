// permissions.d.ts: state() reads, request() asks with at most one dialog.
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export const permissionsHandler: HandlerFactory = (_env, grants) => ({
  async call(method, args) {
    if (method === "state") {
      if (args[0] === undefined) return grants.all();
      return typeof args[0] === "string" ? grants.state(args[0]) : "unavailable";
    }
    if (method === "request") {
      if (args[0] !== undefined && !Array.isArray(args[0])) throw new CapError("invalid_argument", "request takes an array of capability names, or nothing");
      const names = args[0] === undefined ? Object.keys(grants.all()) : (args[0] as unknown[]).filter((n): n is string => typeof n === "string");
      await grants.request(names);
      if (args[0] === undefined) return grants.all();
      return Object.fromEntries(names.map(n => [n, grants.state(n)]));
    }
    throw new CapError("capability_removed", `permissions.${method} is not part of this runtime`);
  },
});
