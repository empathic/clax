import { describe, expect, it } from "vitest";
import { elapsed, hasElapsedLabel, waitingLabel } from "./waiting";

const now = new Date("2026-09-29T10:01:15.000Z");
const since = "2026-09-29T10:00:00.000Z";
const s = (state: any, tier: any, extra = {}) => ({ thread_id: "t", state, tier, since, resends: 0, exhausted: false, ...extra });

describe("waitingLabel", () => {
  it("says nothing for a thread never sent", () => { expect(waitingLabel(null, now)).toBeNull(); });
  it("names the tier being waited on while sent", () => {
    expect(waitingLabel(s("sent", "stop_hook"), now)).toBe("sent, waiting for the agent · 1 min 15 s · waiting on the end of its turn");
    expect(waitingLabel(s("sent", "piggyback"), now)).toBe("sent, waiting for the agent · 1 min 15 s · waiting on its next artifax tool call");
    expect(waitingLabel(s("sent", "queue"), now)).toBe("sent, waiting for the agent · 1 min 15 s · waiting on Codex to pick up the queued message");
    expect(waitingLabel(s("sent", "inject"), now)).toBe("sent, waiting for the agent · 1 min 15 s · waiting on Pi to take the message");
  });
  it("reports delivery, resends, and exhaustion", () => {
    expect(waitingLabel(s("delivered", "queue"), now)).toBe("delivered via codex queue · 1 min 15 s ago · not yet acknowledged");
    expect(waitingLabel(s("delivered", "stop_hook", { resends: 1 }), now)).toBe("delivered via the Stop hook · 1 min 15 s ago · not yet acknowledged · resent once");
    expect(waitingLabel(s("delivered", "stop_hook", { resends: 2 }), now)).toBe("delivered via the Stop hook · 1 min 15 s ago · not yet acknowledged · resent 2 times");
    expect(waitingLabel(s("delivered", "stop_hook", { resends: 3, exhausted: true }), now)).toBe("delivered, not acknowledged");
  });
  it("reports acknowledgement and an ended agent", () => {
    expect(waitingLabel(s("acknowledged", "wait"), now)).toBe("seen by the agent");
    expect(waitingLabel(s("agent_ended", null), now)).toBe("agent session ended; waiting for a new one");
  });
});

it("formats elapsed time", () => {
  expect(elapsed(since, new Date("2026-09-29T10:00:09.400Z"))).toBe("9 s");
  expect(elapsed(since, new Date("2026-09-29T12:05:00.000Z"))).toBe("2 h 5 min");
  expect(elapsed(since, new Date("2026-09-29T09:00:00.000Z"))).toBe("0 s");
});

it("ticks only for labels that show elapsed time", () => {
  expect(hasElapsedLabel(null)).toBe(false);
  expect(hasElapsedLabel(s("sent", "stop_hook"))).toBe(true);
  expect(hasElapsedLabel(s("delivered", "queue"))).toBe(true);
  expect(hasElapsedLabel(s("delivered", "queue", { resends: 3, exhausted: true }))).toBe(false);
  expect(hasElapsedLabel(s("acknowledged", "wait"))).toBe(false);
  expect(hasElapsedLabel(s("agent_ended", null))).toBe(false);
});
