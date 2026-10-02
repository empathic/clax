// window.claude.use(name) (claude.d.ts): a memoised promise per name that
// resolves the capability's frozen namespace or null, never during the page's
// first synchronous run, and never rejects.
// The page-side members come from `opts.locals` (a lazy part in the bridge:
// `caps` for most capabilities, which returns members that `buildNamespace`
// completes, or the capability's own part, which returns the finished
// namespace); one that cannot load resolves null, as a refused capability does.
import { ALIASES, type Local, type UsableName, buildNamespace, isCapabilityName, isPartCapability } from "./capabilities";
import type { Rpc } from "./rpc";

export function makeUse(opts: { framed: boolean; rpc: Rpc; locals: (name: UsableName, rpc: Rpc, config: unknown) => Promise<unknown> }): (name: string) => Promise<unknown> {
  const promises = new Map<string, Promise<unknown>>();
  return function use(name: string): Promise<unknown> {
    const key = typeof name !== "string" ? "" : Object.prototype.hasOwnProperty.call(ALIASES, name) ? ALIASES[name] : name;
    const cached = promises.get(key);
    if (cached) return cached;
    const p = (async () => {
      await Promise.resolve();
      if (!opts.framed || !isCapabilityName(key)) return null;
      const grant = await opts.rpc.use(key);
      if (!grant) return null;
      const local = await opts.locals(key, opts.rpc, grant.config);
      return isPartCapability(key) ? local : buildNamespace(key, opts.rpc, local as Local);
    })().catch(() => null);
    promises.set(key, p);
    return p;
  };
}
