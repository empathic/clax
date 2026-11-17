import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "./api";
import { COMMENTS_OFF, LOAD_FAILED, NAME_FAILED, NAME_LOAD_FAILED, POST_FAILED, SEND_FAILED, failureText, report, scopedNotice } from "./failure";

afterEach(() => { vi.restoreAllMocks(); });

describe("report", () => {
  it("shows the prefixed reason on failure and clears it on success", async () => {
    const notices: (string | null)[] = [];
    const set = (t: string | null) => notices.push(t);
    expect(await report(Promise.reject(new ApiError(500, "boom", "internal")), SEND_FAILED, set)).toBeUndefined();
    expect(await report(Promise.reject(new TypeError("Failed to fetch")), SEND_FAILED, set)).toBeUndefined();
    expect(await report(Promise.resolve(7), SEND_FAILED, set)).toBe(7);
    expect(notices).toEqual(["Could not send to the agent: boom", "Could not send to the agent: Clax did not answer.", null]);
  });
  it("says why by kind, never a status number or a browser's own words", () => {
    const said = (e: unknown) => failureText(POST_FAILED, e);
    const cases: [unknown, string][] = [
      [new ApiError(530, ""), "Could not post: this Clax is not answering. It may have been taken down or moved."],
      [new SyntaxError("Unexpected token '<', \"<!doctype \"... is not valid JSON"), "Could not post: this Clax is not answering. It may have been taken down or moved."],
      [new TypeError("Failed to fetch"), "Could not post: Clax did not answer."],
      [new ApiError(401, "sign in to comment", "sign_in_required"), "Could not post: you are signed out."],
      [new ApiError(404, "not found", "not_found"), "Could not post: it was deleted."],
      [new ApiError(429, "slow down", "rate_limited"), "Could not post: too many requests. Try again in a moment."],
      [new ApiError(500, "database is locked", "internal"), "Could not post: database is locked"],
      [new ApiError(403, "spectators cannot comment", "spectator"), "Could not post: spectators cannot comment"],
      [new ApiError(401, "name yourself to comment", "guest_name_required"), "Could not post: name yourself to comment"],
    ];
    for (const [e, text] of cases) {
      expect(said(e)).toBe(text);
      expect(said(e)).not.toMatch(/\b\d{3}\b|Failed to fetch|Unexpected token|ApiError/);
    }
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    expect(said(new TypeError("Failed to fetch"))).toBe("Could not post: you are offline.");
    expect(said(new ApiError(530, ""))).toBe("Could not post: you are offline.");
  });
  it("says a comments_off refusal as the one comments-off sentence, after its prefix", () => {
    expect(COMMENTS_OFF).toBe("Comments are off for this page");
    expect(failureText("Could not post", new ApiError(403, "comments are off on this artifact", "comments_off"))).toBe("Could not post: comments are off for this page");
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
