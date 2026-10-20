import { afterEach, describe, expect, it } from "vitest";
import { REQUEST_STUCK, STREAM_DOWN, connNoticeText, connNotices, connTrouble } from "./conn-notice";

afterEach(() => {
  connTrouble("stream", false, STREAM_DOWN);
  connTrouble("artifact", false, REQUEST_STUCK);
  connNotices(true);
});

describe("connection notices", () => {
  it("shows the latest reason's words while any holds", () => {
    connTrouble("stream", true, STREAM_DOWN);
    connTrouble("artifact", true, REQUEST_STUCK);
    expect(connNoticeText()).toBe(REQUEST_STUCK);
    connTrouble("artifact", false, REQUEST_STUCK);
    expect(connNoticeText()).toBe(STREAM_DOWN);
    connTrouble("stream", false, STREAM_DOWN);
    expect(connNoticeText()).toBeNull();
  });

  it("shows nothing while turned off, and hides what showed", () => {
    connTrouble("artifact", true, REQUEST_STUCK);
    connNotices(false);
    expect(connNoticeText()).toBeNull();
    connTrouble("stream", true, STREAM_DOWN);
    expect(connNoticeText()).toBeNull();
  });
});
