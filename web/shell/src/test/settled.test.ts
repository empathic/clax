import { afterEach, describe, expect, it, vi } from "vitest";
import { settled } from "./settled";

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("settled", () => {
  it("waits for a chain of zero-delay hops, an animation frame and a lazy import", async () => {
    const seen: string[] = [];
    setTimeout(() => setTimeout(() => setImmediate(() => { seen.push("hops"); }), 0), 0);
    requestAnimationFrame(() => { seen.push("frame"); });
    void import("./timeouts").then(() => { seen.push("import"); });
    await settled();
    expect(seen.sort()).toEqual(["frame", "hops", "import"]);
  });

  it("waits for an answered request's body and what follows it, not for a request held unanswered", async () => {
    let answer!: (r: Response) => void;
    vi.stubGlobal("fetch", vi.fn((url: string) => url === "/held" ? new Promise<Response>(r => { answer = r; }) : Promise.resolve(new Response('{"n":1}'))));
    const got: unknown[] = [];
    void fetch("/now").then(r => r.json()).then(b => setTimeout(() => { got.push(b); }, 0));
    void fetch("/held").then(r => r.json()).then(b => { got.push(b); });
    await settled();
    expect(got).toEqual([{ n: 1 }]);
    answer(new Response('{"n":2}'));
    await settled();
    expect(got).toEqual([{ n: 1 }, { n: 2 }]);
  });

  it("does not wait for a timer with a delay", async () => {
    let fired = false;
    const id = setTimeout(() => { fired = true; }, 60_000);
    await settled();
    expect(fired).toBe(false);
    clearTimeout(id);
  });

  it("returns under fake timers, leaving their timers to an advance", async () => {
    vi.useFakeTimers();
    let fired = false;
    setTimeout(() => { fired = true; }, 0);
    await settled();
    expect(fired).toBe(false);
    await vi.advanceTimersByTimeAsync(0);
    expect(fired).toBe(true);
  });
});
