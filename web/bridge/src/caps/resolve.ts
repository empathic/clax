// What `use(name)` resolves of the framing shell's capabilities (use.ts), in
// the lazy `caps` part: granted by the shell, their page-side members from
// localsFor (index.ts), which `buildNamespace` completes with calls to the
// shell, or, for `sample`, its own part's finished namespace. The page's own
// origin's capabilities are the `origin` part's (origin.ts), and a page that
// is not framed never loads this part.
import { type Local, buildNamespace, isCapabilityName } from "../capabilities";
import type { SamplePart } from "../parts/types";
import type { Rpc } from "../rpc";
import type { CapsEnv } from "./comments";
import { localsFor } from "./index";

export type UseContext = {
  rpc: Rpc;
  sample(): Promise<SamplePart>;
  env: CapsEnv;
};

export async function resolveUse(name: string, o: UseContext): Promise<unknown> {
  if (!isCapabilityName(name)) return null;
  const grant = await o.rpc.use(name);
  if (!grant) return null;
  if (name === "sample") return (await o.sample()).sampleNamespace(o.rpc);
  // `home`'s namespace is whole as caps/home.ts builds it.
  if (name === "home") return localsFor("home", o.rpc, grant.config, o.env);
  return buildNamespace(name as Exclude<typeof name, "room" | "sample" | "home">, o.rpc, localsFor(name as never, o.rpc, grant.config, o.env) as Local);
}
