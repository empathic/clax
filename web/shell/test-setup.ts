import { afterAll, afterEach, expect, vi } from "vitest";

// Node 25 and later define a global `localStorage` that is undefined unless
// Node runs with --localstorage-file (reading it warns), and it shadows
// jsdom's. Tests get the page's storage, as a browser has.
const page = (globalThis as { jsdom?: { window: Window } }).jsdom?.window;
if (page) Object.defineProperty(globalThis, "localStorage", { value: page.localStorage, configurable: true, writable: true });

// Every interval a test file starts must be stopped by the file's end: one
// left running fires after the test environment is torn down, where the DOM
// globals it reads no longer exist, and the run fails on an unhandled error.
const open = new Map<unknown, string>();
const setI = window.setInterval.bind(window);
const clearI = window.clearInterval.bind(window);
window.setInterval = ((f: TimerHandler, ms?: number, ...a: unknown[]) => {
  const id = setI(f, ms, ...a);
  open.set(id, new Error().stack ?? "");
  return id;
}) as typeof window.setInterval;
window.clearInterval = ((id?: number) => { open.delete(id); clearI(id); }) as typeof window.clearInterval;

// A lazy import a test starts (a component, a module loaded after the first
// paint) may still be loading when the test ends. Every test waits for them,
// so none lands after the next test resets the module registry, where it
// would evaluate against a half-loaded copy of the Svelte runtime.
afterEach(async () => { await vi.dynamicImportSettled(); });

afterAll(async () => {
  // The gesture module watches the shell document from the moment it loads;
  // stop the watch of the instance this file's tests used. A file that resets
  // the module registry stops the watch of each instance it loaded itself.
  (await vi.importActual<typeof import("./src/caps/gesture")>("./src/caps/gesture")).unwatchShell();
  expect([...open.values()], "intervals still running").toEqual([]);
});
