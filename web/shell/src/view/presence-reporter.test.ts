import { afterEach, describe, expect, it, vi } from "vitest";
import type { ViewState } from "./artifact-controller";
import { PresenceReporter } from "./presence-reporter";

const AID = "7q3k9mzx2b4t";
let bodies: Record<string, unknown>[] = [];

function stub(focused: boolean) {
  bodies = [];
  vi.stubGlobal("fetch", vi.fn(async (_u: string, init?: RequestInit) => {
    if (init?.body) bodies.push(JSON.parse(String(init.body)));
    return new Response(JSON.stringify({ people: [], viewing: 1 }));
  }));
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
  return vi.spyOn(document, "hasFocus").mockReturnValue(focused);
}
const clock = () => (globalThis as unknown as { claxTestClock: { advance(ms: number): void } }).claxTestClock;
// Every reporter is disposed after its test, so one a failed test leaves cannot report into the next.
const made: PresenceReporter[] = [];
const reporter = () => {
  const r = new PresenceReporter(AID, () => ({ shareWhere: false }) as unknown as ViewState, () => true, () => {});
  made.push(r);
  return r;
};

afterEach(() => { for (const r of made.splice(0)) r.dispose(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("PresenceReporter", () => {
  it("reports focus and idle time, never a timestamp", async () => {
    stub(true);
    const r = reporter();
    dispatchEvent(new Event("keydown"));
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    expect(bodies[0]).toMatchObject({ artifact_id: AID, state: "here", focused: true });
    expect(typeof bodies[0].idle_ms).toBe("number");
    expect(Object.keys(bodies[0]).sort()).toEqual(["artifact_id", "focused", "idle_ms", "state", "tab"]);
    r.dispose();
  });

  it("leaves idle_ms out until the first input: a page opened in the background has none", async () => {
    stub(false);
    const r = reporter();
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    expect(bodies[0]).toMatchObject({ state: "here", focused: false });
    expect("idle_ms" in bodies[0]).toBe(false);
    dispatchEvent(new Event("keydown"));
    clock().advance(2000);
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].idle_ms).toBeGreaterThanOrEqual(2000);
    r.dispose();
  });

  it("counts a pointer move toward idle_ms only while the window has focus", async () => {
    const focus = stub(false);
    const r = reporter();
    // The pointer crosses a Clax window without focus, on its way elsewhere.
    dispatchEvent(new Event("pointermove"));
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    expect("idle_ms" in bodies[0]).toBe(false);
    expect(bodies[0].state).toBe("here");
    focus.mockReturnValue(true);
    clock().advance(2000);
    dispatchEvent(new Event("pointermove"));
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].idle_ms).toBeGreaterThanOrEqual(0);
    expect(bodies[1].idle_ms).toBeLessThan(1000);
    r.dispose();
  });

  it("watches a frame that already holds focus as it starts", async () => {
    const focus = stub(true);
    vi.spyOn(document, "activeElement", "get").mockReturnValue(document.createElement("iframe"));
    const r = reporter();
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    // The viewer leaves for another app with no blur in the shell window.
    focus.mockReturnValue(false);
    clock().advance(1000);
    await vi.waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].focused).toBe(false);
    r.dispose();
  });

  it("reports at once when the tab gains or loses focus", async () => {
    const focus = stub(true);
    const r = reporter();
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    focus.mockReturnValue(false);
    dispatchEvent(new Event("blur"));
    await vi.waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].focused).toBe(false);
    focus.mockReturnValue(true);
    dispatchEvent(new Event("focus"));
    await vi.waitFor(() => expect(bodies.length).toBe(3));
    expect(bodies[2].focused).toBe(true);
    r.dispose();
  });

  it("reads focus again every second while it sits in the frame, and reports when it is lost", async () => {
    const focus = stub(true);
    const frame = document.createElement("iframe");
    const active = vi.spyOn(document, "activeElement", "get").mockReturnValue(frame);
    const r = reporter();
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    // Focus moves into the frame: the shell window hears a blur, but the document keeps focus.
    dispatchEvent(new Event("blur"));
    clock().advance(1000);
    expect(bodies.length).toBe(1);
    // The viewer leaves for another app: the shell window hears nothing.
    focus.mockReturnValue(false);
    clock().advance(1000);
    await vi.waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].focused).toBe(false);
    // Focus leaves the frame: the re-reads stop.
    active.mockReturnValue(document.body);
    focus.mockReturnValue(true);
    clock().advance(1000);
    await vi.waitFor(() => expect(bodies.length).toBe(3));
    expect(bodies[2].focused).toBe(true);
    focus.mockReturnValue(false);
    clock().advance(5000);
    expect(bodies.length).toBe(3);
    r.dispose();
  });

  it("reports the time since the last input as idle_ms", async () => {
    stub(true);
    const r = reporter();
    dispatchEvent(new Event("keydown"));
    clock().advance(5000);
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    expect(bodies[0].idle_ms).toBeGreaterThanOrEqual(5000);
    expect(bodies[0].idle_ms).toBeLessThan(6000);
    // A new input restarts the count (reports go out at most every 2 s).
    dispatchEvent(new Event("keydown"));
    clock().advance(2000);
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].idle_ms).toBeGreaterThanOrEqual(2000);
    expect(bodies[1].idle_ms).toBeLessThan(3000);
    r.dispose();
  });

  it("leaving the page reports away, unfocused, with idle_ms, on a keepalive request", async () => {
    stub(true);
    const r = reporter();
    dispatchEvent(new Event("keydown"));
    dispatchEvent(new Event("pagehide"));
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    expect(bodies[0]).toMatchObject({ artifact_id: AID, state: "away", focused: false });
    expect(bodies[0].idle_ms).toBeGreaterThanOrEqual(0);
    expect(vi.mocked(fetch).mock.calls[0][1]?.keepalive).toBe(true);
    r.dispose();
  });
});
