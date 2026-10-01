// The parts, imported from the daemon's /_clax/bridge/, beside the bridge
// script. The build defines __CLAX_PARTS__ with their content-hashed names,
// so the bridge's own hash (its `?v=`) changes whenever a part does.
//
// Each URL is made when the bridge loads, before any page script runs: the
// path is always /_clax/bridge/<name> on the host that served the bridge, and
// the names are compiled in. `import()` is syntax, so no global the page can
// replace (fetch, URL, document.currentScript, createElement) is on the way.
// Retries, the time limit and the wait for the page's parse are the bridge's
// (part-loader.ts).
import type { CapsPart, ClipPart, CommentPart, Parts } from "./parts/types";

declare const __CLAX_PARTS__: { comment: string; clip: string; caps: string };

/** The parts' loaders; `bridgeSrc` is the bridge script's own URL. */
export function loadParts(bridgeSrc: string): Parts {
  let href: Record<keyof typeof __CLAX_PARTS__, string> | null = null;
  try {
    const base = new URL("/_clax/bridge/", bridgeSrc);
    href = { comment: new URL(__CLAX_PARTS__.comment, base).href, clip: new URL(__CLAX_PARTS__.clip, base).href, caps: new URL(__CLAX_PARTS__.caps, base).href };
  } catch { /* no URL to load from: every part fails */ }
  const at = (name: keyof typeof __CLAX_PARTS__) => href ? href[name] : null;
  // A browser keeps a failed module load for its URL, so a retry asks for
  // the same file under a query naming the attempt.
  const load = <T>(name: keyof typeof __CLAX_PARTS__) => (attempt = 0) => {
    const u = at(name);
    return u ? import(/* @vite-ignore */ attempt ? `${u}?retry=${attempt}` : u) as Promise<T> : Promise.reject(new Error(`no URL for the ${name} part`));
  };
  return { comment: load<CommentPart>("comment"), clip: load<ClipPart>("clip"), caps: load<CapsPart>("caps") };
}
