// Page-side members of each capability the framing shell serves; the rest
// are plain shell calls. (`db`, `assets`, `user` and `room` are the page's
// own origin's: origin.ts.)
import type { CapabilityName, Local } from "../capabilities";
import type { Rpc } from "../rpc";
import { artifactLocals } from "./artifact";
import { type CapsEnv, commentsLocals } from "./comments";
import { downloadsLocals } from "./downloads";
import { homeNamespace } from "./home";
import { locationLocals } from "./location";
import { netLocals } from "./net";

export function localsFor(name: CapabilityName | "home", rpc: Rpc, config: unknown, env: CapsEnv): Local {
  switch (name) {
    case "artifact":
      return artifactLocals(rpc);
    case "comments":
      return commentsLocals(rpc, config, env);
    case "downloads":
      return downloadsLocals(rpc);
    case "net":
      return netLocals(rpc);
    case "location":
      return locationLocals(rpc);
    case "home":
      return homeNamespace(rpc) as unknown as Local;
    default:
      return {};
  }
}
