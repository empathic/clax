import { describe, expect, it, vi } from "vitest";

// A browser test's page (crates/clax-bot/src/browser/session.rs): the
// driver's preamble names the top-level window `clax-test` before any
// script runs, and the bridge gives that window, and no other, a hook that
// runs the artifact's saved tests with the test part's harness.

type Hooked = Window & { claxTest?: { run(o: { files: string[]; testMs?: number; totalMs?: number }): Promise<unknown> } };
const shell = { postMessage: () => {} };

/** Loads the bridge afresh into this document, as the first bridge tag of a
 * page whose window is named `name`, framed by a shell or not. */
async function load(name: string, framed: boolean) {
  vi.resetModules();
  document.querySelectorAll("script[data-artifact]").forEach(s => s.remove());
  Object.defineProperty(window, "parent", { value: framed ? shell : window, configurable: true });
  window.name = name;
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html"; script.dataset.origin = location.origin;
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
}

describe("the bridge's clax-test hook", () => {
  it("the_clax_test_window_gets_the_hook_and_others_do_not", async () => {
    // A second bridge in one window cannot define window.claude again.
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await load("clax-test", true);
    expect((window as Hooked).claxTest).toBeUndefined();
    await load("clax-inspect", false);
    expect((window as Hooked).claxTest).toBeUndefined();
    await load("clax-test", false);
    const hook = (window as Hooked).claxTest;
    expect(hook && typeof hook.run).toBe("function");
    expect(Object.isFrozen(hook)).toBe(true);
    // A page script cannot replace it.
    expect(() => { (window as Hooked).claxTest = { run: async () => ({ passed: 99 }) }; }).toThrow();
    expect((window as Hooked).claxTest).toBe(hook);
  });

  it("the_hook_answers_test_results", async () => {
    // The window the first test left: top level, named clax-test.
    const hook = (window as Hooked).claxTest!;
    expect(await hook.run({ files: [], testMs: 5000, totalMs: 30_000 })).toEqual({ passed: 0, failed: [], skipped: 0 });
    // The harness's globals are the page's now, as the inspect view's are.
    expect(typeof (window as unknown as { test?: unknown }).test).toBe("function");
    expect(typeof (window as unknown as { expect?: unknown }).expect).toBe("function");
    // Files that are not a list of paths run nothing.
    expect(await hook.run({ files: "tests/a.test.js" as unknown as string[] })).toEqual({ passed: 0, failed: [], skipped: 0 });
  });
});
