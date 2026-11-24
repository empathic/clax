import { afterEach, describe, expect, it } from "vitest";
import { KEPT, NOT_CLAX, OFFLINE, REQUEST_STUCK, STREAM_DOWN, UNREACHABLE, UNREACHABLE_HERE, connKept, connNoticeText, connTrouble, noticeFor } from "./conn-notice";

afterEach(() => {
  for (const r of ["stream", "artifact", "api"]) connTrouble(r, false, "");
  connKept("composer", false);
});

const el = () => document.querySelector<HTMLElement>(".conn-notice")!;

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

  it("words each kind plainly, and tells a local daemon from a hosted one", () => {
    expect(noticeFor("not_clax")).toEqual({ text: "This Clax is not answering. It may have been taken down or moved.", rank: 3 });
    expect(noticeFor("offline").text).toBe("You are offline. Clax will catch up when you are back.");
    expect(noticeFor("unreachable", "pr-115.clax-preview.dev").text).toBe("Cannot reach Clax. Retrying…");
    for (const host of ["127.0.0.1", "localhost", "[::1]", "a1.localhost"]) expect(noticeFor("unreachable", host).text).toBe("Clax is not answering on this computer. Retrying…");
    expect([NOT_CLAX, OFFLINE, UNREACHABLE, UNREACHABLE_HERE].every(t => !/\d/.test(t))).toBe(true);
  });

  it("puts an answer that is not Clax's over no answer, and either over the stream alone, whatever came last", () => {
    connTrouble("api", true, NOT_CLAX, document, 3);
    connTrouble("artifact", true, UNREACHABLE, document, 2);
    connTrouble("stream", true, STREAM_DOWN);
    expect(connNoticeText()).toBe(NOT_CLAX);
    connTrouble("api", false, "");
    expect(connNoticeText()).toBe(UNREACHABLE);
    connTrouble("artifact", false, "");
    expect(connNoticeText()).toBe(STREAM_DOWN);
  });

  it("says what was written is kept while a box holds unsent text", () => {
    connTrouble("api", true, NOT_CLAX, document, 3);
    connKept("composer", true);
    expect(connNoticeText()).toBe(`${NOT_CLAX} ${KEPT}`);
    expect(KEPT).toBe("What you wrote is kept.");
    connKept("composer", false);
    expect(connNoticeText()).toBe(NOT_CLAX);
  });

  it("gives an answer that is not Clax's the warning's weight, stays a status and lets clicks through", () => {
    connTrouble("stream", true, STREAM_DOWN);
    expect(el().hasAttribute("data-warn")).toBe(false);
    connTrouble("api", true, NOT_CLAX, document, 3);
    expect(el().hasAttribute("data-warn")).toBe(true);
    expect(el().getAttribute("role")).toBe("status");
    connTrouble("api", false, "");
    expect(el().hasAttribute("data-warn")).toBe(false);
  });
});
