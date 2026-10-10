import { describe, expect, it } from "vitest";
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { needsTicking, pageLabel, sidebarSections } from "./sidebar-model";

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
  it("leads Open with threads about the whole artifact, unnumbered and never Detached", () => {
    const whole = (id: string) => ({ ...t(id, ""), anchor: { kind: "artifact", file: "" } }) as unknown as Thread;
    const s = sidebarSections([t("a"), whole("w"), t("c", "about.html")], { w: lost }, "index.html", f => f === "index.html" || f === "about.html");
    expect(s.open.map(x => x.id)).toEqual(["w", "a", "c"]);
    expect(s.detached).toEqual([]);
    expect([...s.numbers]).toEqual([["a", 1]]);
    expect(sidebarSections([whole("w")], {}, "about.html").open.map(x => x.id)).toEqual(["w"]);
  });
  it("defaults the page to the index when none is given", () => {
    expect(sidebarSections([t("a")], {}, undefined).file).toBe("index.html");
  });
  it("ticks only while an open thread sent to the agent shows elapsed time", () => {
    expect(needsTicking([t("a")])).toBe(false);
    expect(needsTicking([t("a", "index.html", { sent_to_agent: true, feedback_state: { thread_id: "a", state: "sent", tier: null, since: "2026-09-29T00:00:00Z", resends: 0, exhausted: false } } as Partial<Thread>)])).toBe(true);
  });
});

describe("pageLabel", () => {
  it("decodes a percent-encoded page so people can read it", () => {
    expect(pageLabel("?path=src%2Fgraph_mutations.rs&from=2f00")).toBe("?path=src/graph_mutations.rs&from=2f00");
  });
  it("keeps the page as written when it is not valid percent-encoding", () => {
    expect(pageLabel("?q=100%")).toBe("?q=100%");
    expect(pageLabel("about.html")).toBe("about.html");
  });
});
