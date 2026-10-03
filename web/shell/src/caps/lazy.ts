// A capability handler whose code is a chunk of its own, loaded on the page's
// first call to the capability: the artifact entry never carries it (spec §8,
// time to usable). A chunk that cannot load rejects that call
// `capability_disabled`, and the next call tries again.
import { CapError } from "./errors";
import type { Handler, HandlerFactory } from "./host";

export function lazyHandler(load: () => Promise<HandlerFactory>): HandlerFactory {
  return (env, grants) => {
    let inner: Handler | null = null;
    let dead = false;
    let ready: Promise<Handler | null> | null = null;
    const get = () => ready ??= load().then(
      make => (dead ? null : (inner = make(env, grants))),
      e => {
        ready = null;
        throw new CapError("capability_disabled", `this capability could not load: ${e instanceof Error ? e.message : String(e)}`);
      },
    );
    return {
      async call(method, args) {
        const h = await get();
        if (!h) throw new CapError("capability_disabled", "this view has ended");
        return h.call(method, args);
      },
      onEvent: e => inner?.onEvent?.(e),
      reset: () => inner?.reset?.(),
      leave: () => inner?.leave?.(),
      uiChanged: () => inner?.uiChanged?.(),
      reveal: id => inner?.reveal?.(id) ?? false,
      dispose() {
        dead = true;
        if (inner?.dispose) inner.dispose();
        else inner?.reset?.();
        inner = null;
      },
    };
  };
}
