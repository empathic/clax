// Page-side members of each capability; the rest are plain shell calls.
import type { CapabilityName, Local } from "../capabilities";
import type { Rpc } from "../rpc";
import { artifactLocals } from "./artifact";
import { makeDb } from "./db";
import { downloadsLocals } from "./downloads";

export function localsFor(name: CapabilityName, rpc: Rpc, config: unknown): Local {
  void config;
  switch (name) {
    case "artifact":
      return artifactLocals(rpc);
    case "db":
      return makeDb(rpc) as unknown as Local;
    case "downloads":
      return downloadsLocals(rpc);
    default:
      return {};
  }
}
