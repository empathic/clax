// Page-side members of each capability; the rest are plain shell calls.
import type { CapabilityName, Local } from "../capabilities";
import type { Rpc } from "../rpc";
import { makeDb } from "./db";

export function localsFor(name: CapabilityName, rpc: Rpc, config: unknown): Local {
  void config;
  switch (name) {
    case "db":
      return makeDb(rpc) as unknown as Local;
    default:
      return {};
  }
}
