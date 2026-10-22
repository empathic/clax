// What `use(name)` resolves (use.ts), in the lazy `caps` part. A capability
// of the page's own origin (`db`, `assets`, `user`, `room`) comes from
// origin.ts, framed or top-level. The others are the framing shell's:
// granted by it (null unframed), their page-side members from localsFor
// (index.ts), which `buildNamespace` completes with calls to the shell, or,
// for `sample`, its own part's finished namespace.
import { type Local, buildNamespace, isCapabilityName } from "../capabilities";
import type { RoomPart, SamplePart } from "../parts/types";
import type { Rpc } from "../rpc";
import type { CapsEnv } from "./comments";
import { localsFor } from "./index";
import { type Site, originUse } from "./origin";

export type UseContext = {
  /** Whether a window frames the page (the shell, when it welcomes it). */
  framed: boolean;
  rpc: Rpc;
  room(): Promise<RoomPart>;
  sample(): Promise<SamplePart>;
  env: CapsEnv;
  /** The page's site (tests); by default its own origin's. */
  site?: Site;
};

export async function resolveUse(name: string, o: UseContext): Promise<unknown> {
  if (!isCapabilityName(name)) return null;
  const own = await originUse(name, o.framed && o.rpc, o.room, o.site);
  if (own !== undefined) return own;
  if (!o.framed) return null;
  const grant = await o.rpc.use(name);
  if (!grant) return null;
  if (name === "sample") return (await o.sample()).sampleNamespace(o.rpc);
  return buildNamespace(name as Exclude<typeof name, "room" | "sample">, o.rpc, localsFor(name as never, o.rpc, grant.config, o.env) as Local);
}
