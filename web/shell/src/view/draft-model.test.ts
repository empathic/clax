import { describe, expect, it } from "vitest";
import { type DraftView, bannerText, draftSrc, oldestFirst, parseDiff, tintOf } from "./draft-model";

const draft = (agent: string, over: Partial<DraftView> = {}): DraftView => ({
  agent, harness: "claude", base_version: 3, paths: ["index.html"], note: null, updated_at: "2026-10-12T10:00:00.000Z", token: "A".repeat(22), ...over,
});
const name = (h: string, harness: string) => (h === "a_2222" ? "codex" : harness);

describe("draft model", () => {
  it("builds the draft URL under /_clax/draft/ on the artifact's site origin, with the page and the tint", () => {
    const d = draft("a_1111");
    expect(draftSrc(d, "http://7q3k9mzx2b4t.localhost:7481", "index.html", "2f6f2a")).toBe(`http://7q3k9mzx2b4t.localhost:7481/_clax/draft/a_1111/${d.token}/#tint=%232f6f2a`);
    expect(draftSrc(d, "http://7q3k9mzx2b4t.localhost:7481", "docs/a b.html", "7cc36f"))
      .toBe(`http://7q3k9mzx2b4t.localhost:7481/_clax/draft/a_1111/${d.token}/docs/a%20b.html#tint=%237cc36f`);
  });

  it("takes the theme's agent colour, else the light theme's green", () => {
    expect(tintOf(" #7CC36F ")).toBe("7cc36f");
    expect(tintOf("")).toBe("2f6f2a");
    expect(tintOf("rgb(1, 2, 3)")).toBe("2f6f2a");
  });

  it("reads only a well-formed count from a draft page", () => {
    expect(parseDiff({ type: "clax:draft-diff", changed: 2, marked: true })).toEqual({ n: 2, marked: true });
    expect(parseDiff({ type: "clax:draft-diff", changed: 6000, marked: false })).toEqual({ n: 6000, marked: false });
    for (const bad of [null, "x", { type: "clax:hello" }, { type: "clax:draft-diff", changed: -1 }, { type: "clax:draft-diff", changed: 1.5 }]) {
      expect(parseDiff(bad)).toBeNull();
    }
  });

  it("orders drafts by revision and words the banner for one, several and a shown draft", () => {
    const one = draft("a_1111", { note: "restyling the header" });
    const two = draft("a_2222", { updated_at: "2026-10-12T09:00:00.000Z", paths: ["index.html", "style.css"] });
    expect(oldestFirst([one, two]).map(d => d.agent)).toEqual(["a_2222", "a_1111"]);
    expect(bannerText([], null, null, 3, name)).toBeNull();
    expect(bannerText([one], null, null, 3, name)).toEqual({ kind: "drafting", who: "claude", note: "restyling the header", agent: "a_1111" });
    expect(bannerText([one, two], null, null, 3, name)).toEqual({
      kind: "many", who: "2 agents", agents: [{ agent: "a_2222", who: "codex" }, { agent: "a_1111", who: "claude" }],
    });
    const shown = (changed: { n: number; marked: boolean } | null) => bannerText([one, two], "a_2222", changed, 3, name);
    expect(shown(null)).toEqual({ kind: "showing", who: "codex", base: 3, detail: "2 files", others: [{ agent: "a_1111", who: "claude" }], back: "Back to v3" });
    expect(shown({ n: 1, marked: true })).toMatchObject({ detail: "1 change highlighted" });
    expect(shown({ n: 4, marked: true })).toMatchObject({ detail: "4 changes highlighted" });
    expect(shown({ n: 6000, marked: false })).toMatchObject({ detail: "6000 changes, too many to highlight" });
    // A shown draft that is gone reads as not shown.
    expect(bannerText([one], "a_2222", null, 3, name)).toMatchObject({ kind: "drafting" });
  });
});
