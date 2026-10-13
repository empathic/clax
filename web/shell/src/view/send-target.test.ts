import { afterEach, describe, expect, it } from "vitest";
import { ownTarget } from "./send-menu";
import { defaultTarget, liveAgents, rememberTarget } from "./send-target";

// As the daemon lists them: live first, most recently active first.
const agents = [{ handle: "a_cx", harness: "codex", live: true }, { handle: "a_cl", harness: "claude", live: true }, { handle: "a_old", harness: "pi", live: false }];
afterEach(() => localStorage.clear());

describe("send-target", () => {
  it("names the agent you last chose or sent to while it is live, else none", () => {
    expect(defaultTarget("x", agents)).toBeNull();
    rememberTarget("x", "a_cl");
    expect(defaultTarget("x", agents)).toBe("a_cl");
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", agents)).toBeNull(); // the agent you last sent to has ended
    expect(liveAgents(agents).map(a => a.handle)).toEqual(["a_cx", "a_cl"]);
  });
  it("names no target when no agent is live, so the send goes without to", () => {
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", [{ handle: "a_old", harness: "pi", live: false }])).toBeNull();
    expect(defaultTarget("x", [])).toBeNull();
  });
});

describe("ownTarget (spec 2026-10-08 §10)", () => {
  const SAM = { public_id: "u_sam", name: "Sam", owner: false };
  const ALEX = { public_id: "u_alex", name: "Alex", owner: true };
  // Sam's agent is the most recently active, so the daemon lists it first.
  const mixed = [{ handle: "a_sam", harness: "claude", live: true, person: SAM }, { handle: "a_mine", harness: "codex", live: true, person: ALEX }];
  it("is this viewer's own most recent live agent, never another person's listed first", () => {
    expect(ownTarget(mixed, "u_alex")).toBe("a_mine");
    expect(ownTarget(mixed, "u_sam")).toBe("a_sam");
    // A daemon from before people: the most recently active live agent, as before.
    expect(ownTarget(agents, "u_alex")).toBe("a_cx");
  });
  it("is none when only other people's agents are live, or before this viewer is known", () => {
    expect(ownTarget([mixed[0]], "u_alex")).toBeNull();
    expect(ownTarget(mixed, undefined)).toBeNull();
    expect(ownTarget(mixed, "u_lan")).toBeNull();
  });
});
