import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { LOAD_FAILED, NAME_FAILED, NAME_LOAD_FAILED, SEND_FAILED, failureText, report, scopedNotice } from "./failure";

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

describe("scopedNotice", () => {
  it("shows any failure but clears only a notice raised under one of its prefixes", () => {
    let cur: string | null = null;
    const set = (u: string | null | ((prev: string | null) => string | null)) => { cur = typeof u === "function" ? u(cur) : u; };
    const load = scopedNotice(set, LOAD_FAILED);
    const send = scopedNotice(set, SEND_FAILED);
    load("Could not load comments: 500 db locked");
    send(null);
    expect(cur).toBe("Could not load comments: 500 db locked");
    load(null);
    expect(cur).toBeNull();
    send("Could not send to the agent: 500 boom");
    load(null);
    expect(cur).toBe("Could not send to the agent: 500 boom");
    send(null);
    expect(cur).toBeNull();
    // A name save clears a failed name load as well as a failed save.
    const save = scopedNotice(set, NAME_FAILED, NAME_LOAD_FAILED);
    set("Could not load your name: 500 nope");
    save(null);
    expect(cur).toBeNull();
  });
});
