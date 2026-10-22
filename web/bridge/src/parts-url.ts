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
import { PART_NAMES } from "./part-names";
import type { Parts } from "./parts/types";

declare const __CLAX_PARTS__: Partial<Record<keyof Parts, string>>;

/** The parts' loaders, one for every part; `bridgeSrc` is the bridge
 * script's own URL. A part the build named no file for rejects its loads. */
export function loadParts(bridgeSrc: string): Parts {
  const parts: Record<string, (attempt?: number) => Promise<unknown>> = {};
  for (const name of PART_NAMES) {
    const file = __CLAX_PARTS__[name];
    let u: string | null = null;
    try {
      if (file) u = new URL(file, new URL("/_clax/bridge/", bridgeSrc)).href;
    } catch { /* no URL to load from: the part fails */ }
    // A browser keeps a failed module load for its URL, so a retry asks for
    // the same file under a query naming the attempt.
    parts[name] = (attempt = 0) => u ? import(/* @vite-ignore */ attempt ? `${u}?retry=${attempt}` : u) : Promise.reject(new Error(`no URL for the ${name} part`));
  }
  return parts as Parts;
}
