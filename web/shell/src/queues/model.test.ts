import { describe, expect, it } from "vitest";
import type { Report, ReportPage, ReportState } from "../../../sdk/src/types";
import { type Counts, changeText, countReports, countsText, parseOrigins } from "./model";

const report = (state: ReportState, expires?: string) => ({ state, claim: expires ? { holder_name: "claude", expires_at: expires } : null }) as Report;
const counts = (open: number, claimed: number, resolved: number, more = false): Counts => ({ open, claimed, resolved, more, lapses: null });

describe("parseOrigins", () => {
  it("keeps each line's origin once, skipping blank lines", () => {
    expect(parseOrigins("https://app.example\n\n  https://APP.example/  \r\nhttp://localhost:5173\n")).toEqual({
      origins: ["https://app.example", "http://localhost:5173"], bad: [],
    });
  });

  it("names the lines that are not an http or https origin", () => {
    expect(parseOrigins("https://ok.example\nhttps://x.example/path\napp.example\nftp://f.example\nhttps://a.example?q\nhttps://u:p@b.example").bad)
      .toEqual(["https://x.example/path", "app.example", "ftp://f.example", "https://a.example?q", "https://u:p@b.example"]);
  });
});

describe("countReports", () => {
  it("counts every page's reports by state, following next", async () => {
    const pages: Record<string, ReportPage> = {
      "": { reports: [report("open"), report("claimed"), report("open")], next: "c2" },
      c2: { reports: [report("resolved")], next: null },
    };
    const asked: (string | null)[] = [];
    const c = await countReports(async cursor => { asked.push(cursor); return pages[cursor ?? ""]; });
    expect(asked).toEqual([null, "c2"]);
    expect(c).toEqual({ open: 2, claimed: 1, resolved: 1, more: false, lapses: null });
    expect(countsText(c)).toBe("2 open · 1 claimed · 1 resolved");
  });

  it("keeps the soonest a claim's lease lapses", async () => {
    const c = await countReports(async () => ({
      reports: [report("claimed", "2026-11-02T10:05:00Z"), report("claimed", "2026-11-02T10:01:00Z"), report("open")],
      next: null,
    }));
    expect(c.lapses).toBe(Date.parse("2026-11-02T10:01:00Z"));
  });

  it("stops at the page limit and says there are more", async () => {
    let n = 0;
    const c = await countReports(async () => { n++; return { reports: [report("open")], next: "again" }; }, 3);
    expect(n).toBe(3);
    expect(c).toEqual({ open: 3, claimed: 0, resolved: 0, more: true, lapses: null });
    expect(countsText(c)).toBe("3+ open · 0+ claimed · 0+ resolved");
  });
});

describe("changeText", () => {
  it("says what rose, the first with its noun", () => {
    expect(changeText("Bugs", counts(2, 0, 0), counts(1, 1, 0))).toBe("1 report claimed in Bugs.");
    expect(changeText("Bugs", counts(1, 1, 0), counts(1, 0, 1))).toBe("1 report resolved in Bugs.");
    expect(changeText("Bugs", counts(0, 1, 0), counts(2, 0, 1))).toBe("2 reports filed, 1 resolved in Bugs.");
    expect(changeText("Bugs", counts(0, 1, 0), counts(1, 0, 0))).toBe("1 report open again in Bugs.");
    expect(changeText("Bugs", counts(0, 0, 1), counts(1, 0, 0))).toBe("1 report open again in Bugs.");
  });

  it("says nothing when nothing rose or a count is cut short", () => {
    expect(changeText("Bugs", counts(1, 1, 0), counts(1, 1, 0))).toBeNull();
    expect(changeText("Bugs", counts(1, 0, 0, true), counts(0, 1, 0, true))).toBeNull();
  });
});
