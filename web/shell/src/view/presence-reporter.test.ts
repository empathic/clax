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
const reporter = () => new PresenceReporter(AID, () => ({ shareWhere: false }) as unknown as ViewState, () => true, () => {});

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("PresenceReporter", () => {
  it("reports focus and idle time, never a timestamp", async () => {
    stub(true);
    const r = reporter();
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    expect(bodies[0]).toMatchObject({ artifact_id: AID, state: "here", focused: true });
    expect(typeof bodies[0].idle_ms).toBe("number");
    expect(Object.keys(bodies[0]).sort()).toEqual(["artifact_id", "focused", "idle_ms", "state", "tab"]);
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
});
