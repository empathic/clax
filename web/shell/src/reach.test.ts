import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NOT_CLAX, STILL_WAITING, UNREACHABLE_HERE, connNoticeText } from "./conn-notice";
import { STUCK_MS } from "./lifecycle";
import { PROBE_LABEL, PROBE_PATH, heard, pending } from "./reach";

/** Longer than any backoff wait (lifecycle.ts: 30 s, spread up by 20%). */
const BACKOFF_CAP = 36_000;
const clock = () => (globalThis as unknown as { claxTestClock: { dueIn(l: string): number | null } }).claxTestClock;
const cdn = () => new Response("error code: 1016", { status: 530, headers: { "content-type": "text/plain" } });
const clax = () => new Response(JSON.stringify({ version: "1" }), { headers: { "content-type": "application/json" } });
const setVisibility = (v: DocumentVisibilityState) => {
  Object.defineProperty(document, "visibilityState", { value: v, configurable: true });
  Object.defineProperty(document, "hidden", { value: v === "hidden", configurable: true });
  document.dispatchEvent(new Event("visibilitychange"));
};

let answer: () => Response | Promise<Response>;
const f = vi.fn(async (_u: RequestInfo | URL, _i?: RequestInit) => answer());

beforeEach(() => {
  vi.useFakeTimers();
  answer = cdn;
  f.mockClear();
  vi.stubGlobal("fetch", f);
});
afterEach(() => {
  heard(null);
  setVisibility("visible");
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("reaching Clax", () => {
  it("holds the notice with the kind's words, and asks a cheap route with backoff until Clax answers, then clears it by itself", async () => {
    heard("not_clax");
    expect(connNoticeText()).toBe(NOT_CLAX);
    expect(clock().dueIn(PROBE_LABEL)).toBeGreaterThan(0);
    await vi.advanceTimersByTimeAsync(BACKOFF_CAP);
    expect(f).toHaveBeenCalledWith(PROBE_PATH, expect.objectContaining({ cache: "no-store" }));
    expect(connNoticeText()).toBe(NOT_CLAX);
    // Still not answering: it asks again later.
    expect(clock().dueIn(PROBE_LABEL)).toBeGreaterThan(0);
    answer = clax;
    await vi.advanceTimersByTimeAsync(BACKOFF_CAP);
    expect(connNoticeText()).toBeNull();
    expect(clock().dueIn(PROBE_LABEL)).toBeNull();
  });

  it("follows what the latest answer said, and any answer of Clax's lets the notice go", () => {
    heard("unreachable");
    expect(connNoticeText()).toBe(UNREACHABLE_HERE);
    heard("not_clax");
    expect(connNoticeText()).toBe(NOT_CLAX);
    heard(null);
    expect(connNoticeText()).toBeNull();
    // A refusal of Clax's own is no trouble reaching it.
    heard("refused");
    expect(connNoticeText()).toBeNull();
  });

  it("stops asking while the page is hidden and asks again once it is shown", async () => {
    heard("not_clax");
    setVisibility("hidden");
    expect(clock().dueIn(PROBE_LABEL)).toBeNull();
    await vi.advanceTimersByTimeAsync(BACKOFF_CAP);
    expect(f).not.toHaveBeenCalled();
    answer = clax;
    setVisibility("visible");
    await vi.advanceTimersByTimeAsync(BACKOFF_CAP);
    expect(f).toHaveBeenCalledTimes(1);
    expect(connNoticeText()).toBeNull();
  });

  it("says Clax is not answering while a write is still waiting after the stuck threshold, without ending it", async () => {
    const done = pending();
    await vi.advanceTimersByTimeAsync(STUCK_MS - 1);
    expect(connNoticeText()).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(connNoticeText()).toBe(STILL_WAITING);
    done();
    expect(connNoticeText()).toBeNull();
  });
});
