import { afterAll, afterEach, expect, vi } from "vitest";
import { WAIT_MS } from "./src/test/timeouts";

// `vi.waitFor` and `vi.waitUntil` give up after one second unless told
// otherwise, which a loaded machine can take to load one lazy module. Here
// they wait up to `WAIT_MS`, as the tests' own polls do, so a test fails on
// a condition that never comes rather than on one that comes late. The `vi`
// object outlives a test file, so it is wrapped once.
type Wait = typeof vi.waitFor;
const waits = vi as unknown as { waitFor: Wait; waitUntil: typeof vi.waitUntil; claxWaitsPatient?: true };
if (!waits.claxWaitsPatient) {
  const patient = <F extends Wait | typeof vi.waitUntil>(wait: F): F =>
    ((callback: unknown, options?: number | { timeout?: number; interval?: number }) =>
      (wait as (c: unknown, o: object) => unknown)(callback, typeof options === "number" ? { timeout: options } : { timeout: WAIT_MS, ...options })) as F;
  waits.waitFor = patient(vi.waitFor);
  waits.waitUntil = patient(vi.waitUntil);
  waits.claxWaitsPatient = true;
}

// Node 25 and later define a global `localStorage` that is undefined unless
// Node runs with --localstorage-file (reading it warns), and it shadows
// jsdom's. Tests get the page's storage, as a browser has.
const page = (globalThis as { jsdom?: { window: Window } }).jsdom?.window;
if (page) Object.defineProperty(globalThis, "localStorage", { value: page.localStorage, configurable: true, writable: true });

// No unit test reaches a daemon over a WebSocket (jsdom's needs Node's `ws`,
// which the browser build resolves away, and fails unhandled): a socket the
// code under test opens stays connecting until it is closed. Tests of a
// socket's use give their own (`LiveSocket`'s and the room's `make`).
class NoSocket extends EventTarget {
  static readonly CONNECTING = 0; static readonly OPEN = 1; static readonly CLOSING = 2; static readonly CLOSED = 3;
  readyState = 0;
  onopen = null; onmessage = null; onerror = null; onclose = null;
  constructor(readonly url: string) { super(); }
  send(): void {}
  close(): void { this.readyState = 3; }
}
Object.defineProperty(globalThis, "WebSocket", { value: NoSocket, configurable: true, writable: true });

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

// Every stream watch a test makes must end with the test (a component that
// subscribes must unsubscribe when it goes): a stream left with watchers
// stays linked, and its timers fire after the environment is torn down.
afterEach(() => {
  const streams = (globalThis as { claxWatchedStreams?: Set<{ watching: string[]; open: boolean }> }).claxWatchedStreams;
  const left = streams ? [...streams].filter(s => s.open).map(s => s.watching) : [];
  streams?.clear();
  expect(left, "event streams left open after the test (watched, linked or with a timer pending)").toEqual([]);
});

afterAll(async () => {
  // The gesture module watches the shell document from the moment it loads;
  // stop the watch of the instance this file's tests used. A file that resets
  // the module registry stops the watch of each instance it loaded itself.
  (await vi.importActual<typeof import("./src/caps/gesture")>("./src/caps/gesture")).unwatchShell();
  expect([...open.values()], "intervals still running").toEqual([]);
});
