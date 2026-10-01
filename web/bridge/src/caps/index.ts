// Page-side members of each capability; the rest are plain shell calls.
import type { CapabilityName, Local } from "../capabilities";
import type { Rpc } from "../rpc";
import { artifactLocals } from "./artifact";
import { assetsLocals } from "./assets";
import { type CapsEnv, commentsLocals } from "./comments";
import { makeDb } from "./db";
import { downloadsLocals } from "./downloads";

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
    default:
      return {};
  }
}
