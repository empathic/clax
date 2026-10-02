import { afterEach, describe, expect, it } from "vitest";
import { defaultTarget, liveAgents, rememberTarget } from "./send-target";

// As the daemon lists them: live first, most recently active first.
const agents = [{ handle: "a_cx", harness: "codex", live: true }, { handle: "a_cl", harness: "claude", live: true }, { handle: "a_old", harness: "pi", live: false }];
afterEach(() => localStorage.clear());

describe("send-target", () => {
  it("prefers the agent you last sent to while it is live, then the most recently active live agent", () => {
    expect(defaultTarget("x", agents)).toBe("a_cx");
    rememberTarget("x", "a_cl");
    expect(defaultTarget("x", agents)).toBe("a_cl");
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", agents)).toBe("a_cx"); // the agent you last sent to has ended
    expect(liveAgents(agents).map(a => a.handle)).toEqual(["a_cx", "a_cl"]);
  });
  it("names no target when no agent is live, so the send goes without to", () => {
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", [{ handle: "a_old", harness: "pi", live: false }])).toBeNull();
    expect(defaultTarget("x", [])).toBeNull();
  });
});
