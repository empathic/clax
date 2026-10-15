import { describe, expect, it } from "vitest";
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { authorLabel, needsTicking, pageLabel, sidebarSections } from "./sidebar-model";

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

describe("authorLabel", () => {
  const c = { id: "c", thread_id: "t", author_name: "claude", body: "x", created_at: "t" };
  const agent = (person: string | null, person_name: string) =>
    ({ ...c, author_kind: "agent" as const, via_harness: "claude", agent: { name: "clax-web", person, person_name } });
  const owner = { public_id: "u_alex", display_name: "Alex", created_at: "t", owner: true };
  const sam = { public_id: "u_sam", display_name: "Sam", created_at: "t", person: true };
  const lan = { public_id: "u_lan", display_name: "Lan", created_at: "t" };

  it("names an agent's reply by its name, its person and its harness (spec §11)", () => {
    expect(authorLabel(agent("u_alex", "Alex"), owner)).toBe("clax-web (agent, via claude)");
    expect(authorLabel(agent("u_sam", "Sam"), owner)).toBe("clax-web (Sam's agent, via claude)");
    expect(authorLabel(agent("u_sam", "Sam"), sam)).toBe("clax-web (agent, via claude)");
    expect(authorLabel(agent("u_alex", "Alex"), sam)).toBe("clax-web (Alex's agent, via claude)");
    expect(authorLabel(agent("u_alex", "Alex"), lan)).toBe("clax-web (Alex's agent, via claude)");
    expect(authorLabel(agent("u_alex", "Alex"), null)).toBe("clax-web (Alex's agent, via claude)");
    // The extension's panel is the owner's, by public ID, with no flag.
    expect(authorLabel(agent("u_alex", "Alex"), { public_id: "u_alex", display_name: "Alex", created_at: "" })).toBe("clax-web (agent, via claude)");
    // An owner with no viewer row yet: by the owner's flag.
    expect(authorLabel(agent(null, "The owner"), owner)).toBe("clax-web (agent, via claude)");
  });

  it("keeps the harness for a reply from before agent names, and a person's name", () => {
    expect(authorLabel({ ...c, author_kind: "agent", via_harness: "codex" }, owner)).toBe("codex");
    expect(authorLabel({ ...c, author_kind: "viewer", author_name: "<b>Mia</b>", via_harness: null }, owner)).toBe("<b>Mia</b>");
  });
});
