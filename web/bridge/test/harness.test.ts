import { afterEach, beforeEach, describe, expect as vexpect, it, vi } from "vitest";
import { POLL_MS, answerTests, equal, expect as hexpect, makeUi, runTests } from "../src/harness";
import type { BridgeToShell } from "../src/protocol";

type Globals = { test(name: string, fn: () => unknown): void; expect: typeof hexpect; ui: ReturnType<typeof makeUi> };
const g = () => window as unknown as Globals;
const timers = { setTimeout: (f: () => void, ms: number) => setTimeout(f, ms), clearTimeout: (t: unknown) => clearTimeout(t as ReturnType<typeof setTimeout>) };

/** Deps whose loader runs `modules[path]` (the file's body) for each file URL. */
function deps(modules: Record<string, () => unknown>) {
  const loaded: string[] = [];
  return {
    loaded,
    d: {
      timers,
      now: () => Date.now(),
      load: async (url: string) => {
        const path = new URL(url).pathname.replace(/^\//, "");
        loaded.push(path);
        const body = modules[path];
        if (!body) throw new TypeError(`Failed to fetch dynamically imported module: ${url}`);
        return body();
      },
    },
  };
}

describe("expect", () => {
  it("passes and fails each matcher with words for the agent", () => {
    hexpect(2).toBe(2);
    hexpect({ a: [1, { b: 2 }], c: undefined }).toEqual({ a: [1, { b: 2 }] });
    hexpect("x").toBeTruthy();
    hexpect("hello world").toContain("world");
    hexpect([{ id: 1 }, { id: 2 }]).toContain({ id: 2 });
    hexpect(new Set(["a"])).toContain("a");
    hexpect(3).toBeGreaterThan(2);
    vexpect(() => hexpect(1).toBe(2)).toThrow("1 to be 2");
    vexpect(() => hexpect({ a: 1 }).toBe({ a: 1 })).toThrow('{"a":1} to be {"a":1}');
    vexpect(() => hexpect([1]).toEqual([1, 2])).toThrow("[1] to equal [1,2]");
    vexpect(() => hexpect(0).toBeTruthy()).toThrow("0 to be truthy");
    vexpect(() => hexpect("abc").toContain("d")).toThrow('"abc" to contain "d"');
    vexpect(() => hexpect(5).toContain(5)).toThrow("to contain 5");
    vexpect(() => hexpect("3").toBeGreaterThan(2)).toThrow('"3" to be greater than 2');
  });

  it("compares dates by time and arrays apart from objects", () => {
    vexpect(equal(new Date(5), new Date(5))).toBe(true);
    vexpect(equal(new Date(5), new Date(6))).toBe(false);
    vexpect(equal([], {})).toBe(false);
    vexpect(equal({ a: 1 }, { a: 1, b: 2 })).toBe(false);
    vexpect(equal(Number.NaN, Number.NaN)).toBe(true);
  });
});

describe("ui", () => {
  beforeEach(() => { document.body.innerHTML = ""; window.location.hash = ""; });
  const ui = () => makeUi(window, { timers, now: () => Date.now() });

  it("clicks by selector or by text, preferring a control, and says when nothing matches", async () => {
    document.body.innerHTML = `<h2>Add</h2><ul><li>Milk</li></ul><button id="b">  Add </button><a href="#x">Add item now</a>`;
    const seen: string[] = [];
    document.getElementById("b")!.addEventListener("click", () => seen.push("button"));
    document.querySelector("a")!.addEventListener("click", e => { e.preventDefault(); seen.push("link"); });
    document.querySelector("li")!.addEventListener("click", () => seen.push("li"));
    await ui().click("Add");
    await ui().click("#b");
    await ui().click("Milk");
    await ui().click("item now");
    vexpect(seen).toEqual(["button", "button", "li", "link"]);
    await vexpect(ui().click("Nope")).rejects.toThrow('nothing on the page matches or reads "Nope"');
  });

  it("types into a field with input and change events, reads text, and checks existence", async () => {
    document.body.innerHTML = `<input id="n"><p class="out">Total: <b>0</b></p>`;
    const input = document.getElementById("n") as HTMLInputElement;
    const events: string[] = [];
    input.addEventListener("input", () => { events.push("input"); document.querySelector(".out b")!.textContent = input.value; });
    input.addEventListener("change", () => events.push("change"));
    await ui().type("#n", "42");
    vexpect(input.value).toBe("42");
    vexpect(events).toEqual(["input", "change"]);
    vexpect(ui().text(".out")).toBe("Total: 42");
    vexpect(ui().text()).toContain("Total: 42");
    vexpect(ui().exists(".out")).toBe(true);
    vexpect(ui().exists(".missing")).toBe(false);
    vexpect(() => ui().text(".missing")).toThrow('no element matches ".missing"');
    await vexpect(ui().type(".out", "x")).rejects.toThrow("is not a field");
  });

  it("waitFor returns the first truthy value, or gives up after its limit with the last error", async () => {
    let n = 0;
    vexpect(await ui().waitFor(() => (++n >= 3 ? "ready" : null), 1000)).toBe("ready");
    vexpect(n).toBe(3);
    vi.useFakeTimers();
    try {
      const gave = ui().waitFor(() => { throw new Error("not yet"); }, 200);
      const caught = gave.catch((e: Error) => e.message);
      await vi.advanceTimersByTimeAsync(200 + POLL_MS);
      vexpect(await caught).toBe("waitFor gave up after 200 ms: not yet");
    } finally {
      vi.useRealTimers();
    }
  });

  it("go sets the hash, with or without its #", async () => {
    await ui().go("settings");
    vexpect(window.location.hash).toBe("#settings");
    await ui().go("#/list/2");
    vexpect(window.location.hash).toBe("#/list/2");
  });
});

describe("runTests", () => {
  afterEach(() => { vi.useRealTimers(); document.body.innerHTML = ""; });

  it("loads each file with the globals in place and runs its tests in order", async () => {
    const order: string[] = [];
    const { d, loaded } = deps({
      "tests/a.test.js": () => {
        g().test("adds", () => { order.push("adds"); g().expect(1 + 1).toBe(2); });
        g().test("async fails", async () => { order.push("async"); await Promise.resolve(); g().expect([1]).toEqual([2]); });
      },
      "tests/b.test.js": () => {
        g().test("throws", () => { order.push("throws"); throw new TypeError("x is undefined"); });
        g().test("uses ui", async () => { document.body.innerHTML = "<button>Go</button>"; await g().ui.click("Go"); });
      },
    });
    const r = await runTests(window, ["tests/a.test.js", "tests/b.test.js", "tests/missing.test.js"], {}, d);
    vexpect(loaded).toEqual(["tests/a.test.js", "tests/b.test.js", "tests/missing.test.js"]);
    vexpect(order).toEqual(["adds", "async", "throws"]);
    vexpect(r.passed).toBe(2);
    vexpect(r.skipped).toBe(0);
    vexpect(r.failed).toEqual([
      { name: "tests/missing.test.js", error: vexpect.stringContaining("the file did not load: TypeError: Failed to fetch") },
      { name: "async fails", error: "Expected: [1] to equal [2]" },
      { name: "throws", error: "TypeError: x is undefined" },
    ]);
  });

  it("ends a test at its limit and skips the tests the run has no time left for", async () => {
    vi.useFakeTimers();
    const { d } = deps({
      "tests/a.test.js": () => {
        g().test("hangs", () => new Promise(() => {}));
        g().test("hangs too", () => new Promise(() => {}));
        g().test("never reached", () => {});
      },
    });
    const done = runTests(window, ["tests/a.test.js"], { testMs: 100, totalMs: 150 }, { ...d, now: () => Date.now() });
    await vi.advanceTimersByTimeAsync(200);
    vexpect(await done).toEqual({
      passed: 0,
      failed: [
        { name: "hangs", error: "it took longer than 100 ms" },
        { name: "hangs too", error: "the run's 150 ms ran out" },
      ],
      skipped: 1,
    });
  });

  it("answers the order with its request ID, or why there was no run", async () => {
    const posted: BridgeToShell[] = [];
    const { d } = deps({ "tests/a.test.js": () => g().test("ok", () => {}) });
    await answerTests(window, { type: "clax:test", requestId: "t1", files: ["tests/a.test.js"] }, m => posted.push(m), d);
    await answerTests(window, { type: "clax:test", requestId: "t2", files: [3 as unknown as string] }, m => posted.push(m), d);
    await answerTests(window, { type: "clax:test", requestId: 5 as unknown as string, files: [] }, m => posted.push(m), d);
    vexpect(posted).toEqual([
      { type: "clax:tests", requestId: "t1", passed: 1, failed: [], skipped: 0 },
      { type: "clax:tests", requestId: "t2", error: "files is a list of paths" },
    ]);
  });
});
