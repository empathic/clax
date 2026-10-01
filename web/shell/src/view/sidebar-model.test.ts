import { describe, expect, it } from "vitest";
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { needsTicking, sidebarSections } from "./sidebar-model";

const t = (id: string, file = "index.html", extra: Partial<Thread> = {}) => ({ id, status: "open", anchor: { file }, sent_to_agent: false, feedback_state: null, ...extra }) as unknown as Thread;
const lost = { found: false, method: null, rect: null } as unknown as AnchorResult;

describe("sidebarSections", () => {
  it("orders attached then elsewhere, sends lost and unheld threads to Detached, and numbers only the attached", () => {
    const s = sidebarSections([t("a"), t("b"), t("c", "about.html"), t("d", "gone.html"), t("e", "index.html", { status: "resolved" } as Partial<Thread>)], { b: lost }, "index.html", f => f !== "gone.html");
    expect(s.open.map(x => x.id)).toEqual(["a", "c"]);
    expect(s.detached.map(x => x.id)).toEqual(["b", "d"]);
    expect(s.resolved.map(x => x.id)).toEqual(["e"]);
    expect([...s.numbers]).toEqual([["a", 1]]);
  });
  it("defaults the page to the index when none is given", () => {
    expect(sidebarSections([t("a")], {}, undefined).file).toBe("index.html");
  });
  it("ticks only while an open thread sent to the agent shows elapsed time", () => {
    expect(needsTicking([t("a")])).toBe(false);
    expect(needsTicking([t("a", "index.html", { sent_to_agent: true, feedback_state: { thread_id: "a", state: "sent", tier: null, since: "2026-09-29T00:00:00Z", resends: 0, exhausted: false } } as Partial<Thread>)])).toBe(true);
  });
});
