// window.claude.use(name) (claude.d.ts): a memoised promise per name that
// resolves the capability's frozen namespace or null, never during the page's
// first synchronous run, and never rejects.
// The page-side members come from `opts.locals` (the lazy caps part in the
// bridge); one that cannot load resolves null, as a refused capability does.
import { ALIASES, type CapabilityName, type Local, buildNamespace, isCapabilityName } from "./capabilities";
import type { Rpc } from "./rpc";

export function makeUse(opts: { framed: boolean; rpc: Rpc; locals: (name: CapabilityName, rpc: Rpc, config: unknown) => Promise<Local> }): (name: string) => Promise<unknown> {
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
      return buildNamespace(key, opts.rpc, await opts.locals(key, opts.rpc, grant.config));
    })().catch(() => null);
    promises.set(key, p);
    return p;
  };
}
