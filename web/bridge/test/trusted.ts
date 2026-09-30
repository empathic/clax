// Dispatches a synthetic event as the browser's own (`isTrusted`), as a real
// `postMessage` from the shell arrives: the bridge takes only trusted
// messages. `dispatchEvent` marks every event untrusted, so this dispatches
// through jsdom's internals.
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
type Impl = { isTrusted: boolean; _dispatch(e: Impl): boolean };
const utils = require("jsdom/lib/jsdom/living/generated/utils.js") as { implForWrapper(w: unknown): Impl };

export function dispatchTrusted(target: EventTarget, e: Event): boolean {
  const impl = utils.implForWrapper(e);
  impl.isTrusted = true;
  // oxlint-disable-next-line no-underscore-dangle -- jsdom's own name
  const g = (target as unknown as { _globalObject?: unknown })._globalObject;
  const at = utils.implForWrapper(target) ?? (g && utils.implForWrapper(g));
  // oxlint-disable-next-line no-underscore-dangle -- jsdom's own name
  return at._dispatch(impl);
}

/** `e`, marked trusted, for code that reads it without dispatching it. */
export function trusted<E extends Event>(e: E): E {
  utils.implForWrapper(e).isTrusted = true;
  return e;
}
