import { describe, expect, it } from "vitest";
import { fromAccess, NO_ACCESS, summary, toAccess, withAction, withQueue } from "./queue-access";

describe("queue access", () => {
  it("is no grant until a queue and an action are chosen", () => {
    expect(toAccess(NO_ACCESS)).toBeNull();
    expect(toAccess(withAction(NO_ACCESS, "claim", true))).toBeNull();
    expect(toAccess(withQueue(NO_ACCESS, "q1", true))).toBeNull();
    expect(summary(NO_ACCESS, {})).toBe("No queue access");
  });

  it("adds Read with any action, and Read off turns every action off", () => {
    const c = withAction(withQueue(NO_ACCESS, "q1", true), "fix", true);
    expect(c.actions).toEqual(["read", "fix"]);
    expect(toAccess(c)).toEqual([{ type: "clax-queues", actions: ["read", "fix"], locations: ["q1"] }]);
    expect(withAction(c, "fix", false).actions).toEqual(["read"]);
    expect(withAction(c, "read", false).actions).toEqual([]);
  });

  it("chooses every queue the granter holds as *", () => {
    const c = withAction(withQueue(withQueue(NO_ACCESS, "q1", true), "*", true), "claim", true);
    expect(c).toEqual({ all: true, queues: [], actions: ["read", "claim"] });
    expect(toAccess(c)![0].locations).toEqual(["*"]);
    expect(summary(c, {})).toBe("Read, Claim on all my queues");
    expect(withQueue(c, "q2", true)).toEqual({ all: false, queues: ["q2"], actions: ["read", "claim"] });
  });

  it("reads a stored grant back, naming queues by name", () => {
    const c = fromAccess([{ type: "clax-queues", actions: ["reply", "read"], locations: ["q2", "q1"] }]);
    expect(c).toEqual({ all: false, queues: ["q1", "q2"], actions: ["read", "reply"] });
    expect(summary(c, { q1: "App reports" })).toBe("Read, Reply on App reports, q2");
    expect(fromAccess(null)).toEqual(NO_ACCESS);
    expect(fromAccess([{ type: "clax-comments", actions: ["read"] }])).toEqual(NO_ACCESS);
  });
});
