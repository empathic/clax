// window.claude.use(name) (claude.d.ts): a memoised promise per name that
// resolves the capability's frozen namespace or null, never during the page's
// first synchronous run, and never rejects.
import { ALIASES, buildNamespace, isCapabilityName } from "./capabilities";
import { localsFor } from "./caps";
import type { Rpc } from "./rpc";

export function makeUse(opts: { framed: boolean; rpc: Rpc; locals?: typeof localsFor }): (name: string) => Promise<unknown> {
  const locals = opts.locals ?? localsFor;
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
      return buildNamespace(key, opts.rpc, locals(key, opts.rpc, grant.config));
    })().catch(() => null);
    promises.set(key, p);
    return p;
  };
}
