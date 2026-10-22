// window.claude.use(name) (claude.d.ts): a memoised promise per name that
// resolves the capability's frozen namespace or null, never during the page's
// first synchronous run, and never rejects. What a name resolves is
// `resolve`'s (the lazy `caps` part's `resolveUse`, caps/resolve.ts); a part
// that cannot load resolves null, as a refused capability does.
import { ALIASES } from "./capabilities";

export function makeUse(resolve: (name: string) => Promise<unknown>): (name: string) => Promise<unknown> {
  const promises = new Map<string, Promise<unknown>>();
  return function use(name: string): Promise<unknown> {
    const key = typeof name !== "string" ? "" : Object.prototype.hasOwnProperty.call(ALIASES, name) ? ALIASES[name] : name;
    const cached = promises.get(key);
    if (cached) return cached;
    const p = (async () => {
      await Promise.resolve();
      return resolve(key);
    })().catch(() => null);
    promises.set(key, p);
    return p;
  };
}
