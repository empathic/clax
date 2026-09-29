import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { SEND_FAILED, failureText, report } from "./failure";

describe("report", () => {
  it("shows the prefixed message on failure and clears it on success", async () => {
    const notices: (string | null)[] = [];
    const set = (t: string | null) => notices.push(t);
    expect(await report(Promise.reject(new ApiError(500, "boom")), SEND_FAILED, set)).toBeUndefined();
    expect(await report(Promise.reject(new TypeError("Failed to fetch")), SEND_FAILED, set)).toBeUndefined();
    expect(await report(Promise.resolve(7), SEND_FAILED, set)).toBe(7);
    expect(notices).toEqual(["Could not send to the agent: 500 boom", "Could not send to the agent: Failed to fetch", null]);
  });
  it("formats non-Error values", () => {
    expect(failureText("Could not post", "offline")).toBe("Could not post: offline");
  });
});
