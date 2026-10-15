// Page-side members of each capability; the rest are plain shell calls.
import type { CapabilityName, Local } from "../capabilities";
import type { Rpc } from "../rpc";
import { artifactLocals } from "./artifact";
import { assetsLocals } from "./assets";
import { type CapsEnv, commentsLocals } from "./comments";
import { makeDb } from "./db";
import { downloadsLocals } from "./downloads";
import { locationLocals } from "./location";
import { netLocals } from "./net";

export function localsFor(name: CapabilityName, rpc: Rpc, config: unknown, env: CapsEnv): Local {
  switch (name) {
    case "artifact":
      return artifactLocals(rpc);
    case "assets":
      return assetsLocals(rpc);
    case "comments":
      return commentsLocals(rpc, config, env);
    case "db":
      return makeDb(rpc) as unknown as Local;
    case "downloads":
      return downloadsLocals(rpc);
    case "net":
      return netLocals(rpc);
    case "location":
      return locationLocals(rpc);
    default:
      return {};
  }
}
