// The test harness of the app's inspect view (spec 2026-10-12-flutter-real-
// apps-design C2): the app's agent writes `tests/*.test.js` into a draft, and
// the inspect view, on a load of the draft against a scratch copy of its
// documents, asks the bridge to run them (`clax:test`). Loaded with the test
// part (`parts/test.ts`), only there.
//
// Before the files load, the page's window gets three globals:
// - `test(name, fn)` registers a test; `fn` may be async.
// - `expect(v)` with `toBe` (`Object.is`), `toEqual` (structural, ignoring
//   properties that are undefined), `toBeTruthy`, `toContain` (a substring,
//   or an equal item of an array or other iterable) and `toBeGreaterThan`.
// - `ui`: `click(selectorOrText)`, `type(selector, text)`, `text(selector?)`,
//   `exists(selector)`, `waitFor(fn, ms)` and `go(hash)`; the first two and
//   `go` resolve once the page has had a turn to react.
//
// Each file is imported as a module, in the order given; a file that does not
// load is one failure named by its path. The tests then run one at a time, in
// the order registered, each for at most `testMs`, the whole run (loads
// included) for at most `totalMs`: the tests the run had no time left for are
// skipped.
//
// The same part runs an app bundle's migration (spec 2026-10-13-flutter-app-
// bundles-design §8.3, `clax:migrate`): the file (`migrate.js`) is imported
// as a module, and its default export is called with the page's
// `window.claude.use("db")` and `{from, to}`; the whole run, the load
// included, takes at most `totalMs`.
import type { BridgeToShell, MigrationResult, ShellToBridge, TestResults } from "./protocol";

/** How long one test may take, and the whole run, when the order names no limit. */
export const TEST_MS = 5000;
export const TOTAL_MS = 30_000;
/** The longest limits an order may ask for. */
const TEST_MAX_MS = 30_000;
const TOTAL_MAX_MS = 120_000;
/** `ui.waitFor`'s limit when the test names none, and how often it looks. */
export const WAIT_MS = 2000;
export const POLL_MS = 50;
/** The longest text kept of one failure's error, and how many failures are kept. */
const ERROR_CHARS = 500;
const MAX_FAILED = 50;

export type HarnessTimers = { setTimeout(f: () => void, ms: number): unknown; clearTimeout(t: unknown): void };
export type HarnessDeps = {
  timers: HarnessTimers;
  now(): number;
  /** Imports the module at `url`. */
  load(url: string): Promise<unknown>;
};

type Test = { name: string; fn: () => unknown };

const cut = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);
/** `e` in words; a value that cannot be put in words (its conversion throws) is said to be so. */
const message = (e: unknown) => {
  try {
    return cut(e instanceof Error ? (e.name && e.name !== "Error" ? `${e.name}: ${e.message}` : e.message) : String(e), ERROR_CHARS);
  } catch {
    return "a value that cannot be shown";
  }
};

/** `v` in words for a failed expectation. */
function show(v: unknown): string {
  if (typeof v === "string") return JSON.stringify(cut(v, 100));
  if (v === undefined || typeof v === "function" || typeof v === "symbol" || typeof v === "bigint") return String(v);
  try {
    return cut(JSON.stringify(v) ?? String(v), 100);
  } catch {
    return String(v);
  }
}

/** Whether `a` and `b` hold the same values: arrays item by item, plain and
 * other objects by their own enumerable properties (those set to undefined
 * left out), dates by their time. */
export function equal(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (a instanceof Date || b instanceof Date) return a instanceof Date && b instanceof Date && a.getTime() === b.getTime();
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) return a.length === (b as unknown[]).length && a.every((v, i) => equal(v, (b as unknown[])[i]));
  const keys = (o: object) => Object.keys(o).filter(k => (o as Record<string, unknown>)[k] !== undefined);
  const ka = keys(a);
  const kb = keys(b);
  return ka.length === kb.length && ka.every(k => Object.prototype.hasOwnProperty.call(b, k) && equal((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
}

class ExpectationError extends Error {
  override name = "Expected";
}

/** The `expect(v)` the tests get. */
export function expect(actual: unknown) {
  const check = (ok: boolean, what: string) => { if (!ok) throw new ExpectationError(`${show(actual)} ${what}`); };
  return {
    toBe: (x: unknown) => check(Object.is(actual, x), `to be ${show(x)}`),
    toEqual: (x: unknown) => check(equal(actual, x), `to equal ${show(x)}`),
    toBeTruthy: () => check(!!actual, "to be truthy"),
    toContain: (x: unknown) => {
      let ok = false;
      if (typeof actual === "string") ok = typeof x === "string" && actual.includes(x);
      else if (actual !== null && typeof actual === "object" && Symbol.iterator in actual) ok = Array.from(actual as Iterable<unknown>).some(v => equal(v, x));
      check(ok, `to contain ${show(x)}`);
    },
    toBeGreaterThan: (x: number) => check(typeof actual === "number" && typeof x === "number" && actual > x, `to be greater than ${show(x)}`),
  };
}

const CLICKABLE = "button,a,[role=button],[role=tab],[role=link],[role=menuitem],[role=option],[role=switch],[role=checkbox],label,summary,input[type=button],input[type=submit],input[type=checkbox],input[type=radio]";
const flat = (s: string | null | undefined) => (s ?? "").replace(/\s+/g, " ").trim();

/** Whether a reader sees `el`, where the browser can say. */
function shown(el: Element): boolean {
  const check = (el as Element & { checkVisibility?: (o?: object) => boolean }).checkVisibility;
  return typeof check === "function" ? check.call(el, { visibilityProperty: true }) : true;
}

/** The `ui` the tests get, over `win`. */
export function makeUi(win: Window, deps: Pick<HarnessDeps, "timers" | "now">) {
  const doc = win.document;
  // The page's own constructors: an element of the page is no instance of another realm's.
  const W = win as unknown as typeof globalThis;
  const turn = () => new Promise<void>(r => deps.timers.setTimeout(r, 0));
  /** The element `selector` names, or null; a selector that does not parse names none. */
  const query = (selector: string): Element | null => {
    try {
      return doc.querySelector(selector);
    } catch {
      return null;
    }
  };
  const required = (selector: string): Element => {
    if (typeof selector !== "string") throw new TypeError("a selector is a string");
    const el = query(selector);
    if (!el) throw new Error(`no element matches ${show(selector)}`);
    return el;
  };
  /** The element `s` names: a selector's first match; else a shown control
   * whose text, label or value is `s`; else the innermost shown element whose
   * text is `s`; else a shown control whose text contains it. */
  const find = (s: string): Element => {
    if (typeof s !== "string" || !s) throw new TypeError("click takes a selector or the text of what to click");
    const el = query(s);
    if (el) return el;
    const t = flat(s);
    const controls = [...doc.querySelectorAll(CLICKABLE)].filter(shown);
    const named = (c: Element) => flat(c.textContent) === t || flat(c.getAttribute("aria-label")) === t || ((c as HTMLInputElement).value !== undefined && c.localName === "input" && flat((c as HTMLInputElement).value) === t);
    const exact = controls.find(named);
    if (exact) return exact;
    const all = doc.body ? [...doc.body.querySelectorAll("*")].filter(e => flat(e.textContent) === t && shown(e)) : [];
    if (all.length) return all[all.length - 1];
    const partial = controls.find(c => flat(c.textContent).includes(t));
    if (partial) return partial;
    throw new Error(`nothing on the page matches or reads ${show(s)}`);
  };
  return {
    async click(selectorOrText: string): Promise<void> {
      const el = find(selectorOrText);
      if (typeof (el as HTMLElement).click === "function") (el as HTMLElement).click();
      else el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: win }));
      await turn();
    },
    async type(selector: string, text: string): Promise<void> {
      const el = required(selector) as HTMLElement;
      el.focus?.();
      const value = String(text);
      if (el instanceof W.HTMLInputElement || el instanceof W.HTMLTextAreaElement || el instanceof W.HTMLSelectElement) {
        // The prototype's setter, so a framework that tracks the value sees the change.
        const proto = Object.getPrototypeOf(el) as object;
        const set = Object.getOwnPropertyDescriptor(proto, "value")?.set;
        if (set) set.call(el, value);
        else el.value = value;
      } else if (el.isContentEditable) {
        el.textContent = value;
      } else {
        throw new Error(`${show(selector)} is not a field`);
      }
      el.dispatchEvent(new W.Event("input", { bubbles: true }));
      el.dispatchEvent(new W.Event("change", { bubbles: true }));
      await turn();
    },
    text(selector?: string): string {
      const el = selector === undefined ? doc.body : required(selector);
      if (!el) return "";
      const inner = (el as HTMLElement).innerText;
      return flat(typeof inner === "string" ? inner : el.textContent);
    },
    exists(selector: string): boolean {
      if (typeof selector !== "string") throw new TypeError("a selector is a string");
      return query(selector) !== null;
    },
    async waitFor<T>(fn: () => T | Promise<T>, ms: number = WAIT_MS): Promise<T> {
      if (typeof fn !== "function") throw new TypeError("waitFor takes a function");
      const start = deps.now();
      let last: unknown = null;
      for (;;) {
        try {
          const v = await fn();
          if (v) return v;
          last = null;
        } catch (e) {
          last = e;
        }
        if (deps.now() - start >= ms) throw new Error(`waitFor gave up after ${ms} ms${last ? `: ${message(last)}` : ""}`);
        await new Promise<void>(r => deps.timers.setTimeout(r, POLL_MS));
      }
    },
    async go(hash: string): Promise<void> {
      const h = String(hash);
      win.location.hash = h.startsWith("#") ? h : `#${h}`;
      await turn();
    },
  };
}

const bounded = (v: unknown, lo: number, hi: number, dflt: number) => (typeof v === "number" && Number.isFinite(v) ? Math.min(hi, Math.max(lo, Math.round(v))) : dflt);

/** The error `within` rejects with when time runs out. */
class Late extends Error {}

/** Rejects with `why` (a `Late`) after `ms`, else settles as `p` does. */
function within<T>(p: Promise<T>, ms: number, why: string, t: HarnessTimers): Promise<T> {
  let timer: unknown;
  const late = new Promise<never>((_, reject) => { timer = t.setTimeout(() => reject(new Late(why)), Math.max(0, ms)); });
  return Promise.race([p, late]).finally(() => t.clearTimeout(timer));
}

/** Runs the test files `files` (paths relative to the page, `tests/….test.js`)
 * in `win`, as the module comment says. */
export async function runTests(win: Window, files: string[], opts: { testMs?: number; totalMs?: number }, deps: HarnessDeps): Promise<TestResults> {
  const testMs = bounded(opts.testMs, 1, TEST_MAX_MS, TEST_MS);
  const totalMs = bounded(opts.totalMs, 1, TOTAL_MAX_MS, TOTAL_MS);
  const start = deps.now();
  const left = () => totalMs - (deps.now() - start);
  const tests: Test[] = [];
  const failed: TestResults["failed"] = [];
  const fail = (name: string, error: string) => { if (failed.length < MAX_FAILED) failed.push({ name: cut(name, 200), error }); };
  const globals: Record<string, unknown> = {
    test: (name: unknown, fn: unknown) => {
      if (typeof fn !== "function") throw new TypeError("test takes a name and a function");
      tests.push({ name: String(name), fn: fn as () => unknown });
    },
    expect,
    ui: makeUi(win, deps),
  };
  for (const [k, v] of Object.entries(globals)) Object.defineProperty(win, k, { value: v, configurable: true, writable: true });

  let passed = 0;
  let skipped = 0;
  for (const file of files) {
    if (left() <= 0) {
      fail(file, `not loaded: the run's ${totalMs} ms ran out`);
      continue;
    }
    try {
      await within(deps.load(new URL(file, win.location.href).href), left(), `it took longer than the run's ${totalMs} ms to load`, deps.timers);
    } catch (e) {
      fail(file, `the file did not load: ${message(e)}`);
    }
  }
  for (const t of tests) {
    const ms = Math.min(testMs, left());
    if (ms <= 0) {
      skipped++;
      continue;
    }
    const why = ms < testMs ? `the run's ${totalMs} ms ran out` : `it took longer than ${testMs} ms`;
    try {
      await within(Promise.resolve().then(t.fn), ms, why, deps.timers);
      passed++;
    } catch (e) {
      fail(t.name, message(e));
    }
  }
  return { passed, failed, skipped };
}

/** The bridge's `clax:test` order, answered: `post` gets `clax:tests` with the
 * order's `requestId` and the results, or why there are none. An order
 * without a `requestId`, or whose `files` is not a list of strings, gets no
 * run. */
export async function answerTests(win: Window, m: Extract<ShellToBridge, { type: "clax:test" }>, post: (m: BridgeToShell) => void, deps: HarnessDeps): Promise<void> {
  const requestId = m.requestId;
  if (typeof requestId !== "string") return;
  if (!Array.isArray(m.files) || !m.files.every(f => typeof f === "string")) {
    post({ type: "clax:tests", requestId, error: "files is a list of paths" });
    return;
  }
  try {
    post({ type: "clax:tests", requestId, ...(await runTests(win, m.files, m, deps)) });
  } catch (e) {
    post({ type: "clax:tests", requestId, error: message(e) });
  }
}

/** How long a migration run may take when the order names no limit, and at most. */
export const MIGRATE_MS = 30_000;
const MIGRATE_MAX_MS = 120_000;

export type MigrationDeps = HarnessDeps & {
  /** The page's `db` (`window.claude.use("db")`). */
  db(): Promise<unknown>;
};

/** Runs the migration `file` (a path relative to the page) in `win`, as the
 * module comment says. */
export async function runMigration(win: Window, file: string, opts: { from: number; to: number; totalMs?: number }, deps: MigrationDeps): Promise<MigrationResult> {
  const totalMs = bounded(opts.totalMs, 1, MIGRATE_MAX_MS, MIGRATE_MS);
  const start = deps.now();
  const left = () => totalMs - (deps.now() - start);
  const late = `it took longer than ${totalMs} ms`;
  let fn: unknown;
  try {
    const mod = await within(deps.load(new URL(file, win.location.href).href), left(), late, deps.timers);
    fn = (mod as { default?: unknown } | null)?.default;
  } catch (e) {
    return { ok: false, error: e instanceof Late ? late : `${file} did not load: ${message(e)}` };
  }
  if (typeof fn !== "function") return { ok: false, error: `${file} has no default export function` };
  let db: unknown;
  try {
    db = await within(deps.db(), left(), late, deps.timers);
  } catch (e) {
    return { ok: false, error: e instanceof Late ? late : `the page's db is not available: ${message(e)}` };
  }
  try {
    await within(Promise.resolve().then(() => (fn as (db: unknown, v: { from: number; to: number }) => unknown)(db, { from: opts.from, to: opts.to })), left(), late, deps.timers);
    return { ok: true };
  } catch (e) {
    return { ok: false, error: e instanceof Late ? late : `the migration failed: ${message(e)}` };
  }
}

const version = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 1;

/** The bridge's `clax:migrate` order, answered: `post` gets `clax:migration`
 * with the order's `requestId` and whether the run finished, or why not. An
 * order without a `requestId` gets no run. */
export async function answerMigration(win: Window, m: Extract<ShellToBridge, { type: "clax:migrate" }>, post: (m: BridgeToShell) => void, deps: MigrationDeps): Promise<void> {
  const requestId = m.requestId;
  if (typeof requestId !== "string") return;
  if (typeof m.file !== "string" || !m.file || !version(m.from) || !version(m.to)) {
    post({ type: "clax:migration", requestId, ok: false, error: "file is a path, from and to are versions" });
    return;
  }
  try {
    post({ type: "clax:migration", requestId, ...(await runMigration(win, m.file, m, deps)) });
  } catch (e) {
    post({ type: "clax:migration", requestId, ok: false, error: message(e) });
  }
}
