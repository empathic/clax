/**
 * Runtime bridge injected into every published page.
 * Phase 1 exposes `window.claude.use(name)` and resolves `null` for every
 * capability, so pages written against the claude.ai contract load and
 * degrade correctly. Later phases add the shell handshake.
 */
(() => {
  const script = document.currentScript as HTMLScriptElement | null;
  const meta = {
    artifact: script?.dataset.artifact ?? "",
    version: Number(script?.dataset.version ?? "0"),
    contract: script?.dataset.contract ?? "",
  };
  // oxlint-disable-next-line no-underscore-dangle -- public global read by the shell
  (window as any).__artifax = meta;

  const cache = new Map<string, Promise<null>>();
  function use(name: string): Promise<null> {
    let p = cache.get(name);
    if (!p) {
      p = Promise.resolve().then(() => null);
      cache.set(name, p);
    }
    return p;
  }

  try {
    Object.defineProperty(window, "claude", {
      value: Object.freeze({ use }),
      writable: false,
      configurable: false,
      enumerable: true,
    });
  } catch (e) {
    // A page that already defined a non-configurable window.claude wins.
    console.warn("artifax: could not install window.claude", e);
  }
})();

export {};
