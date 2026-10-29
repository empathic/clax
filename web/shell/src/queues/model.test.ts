import { describe, expect, it } from "vitest";
import type { Report, ReportPage, ReportState } from "../../../sdk/src/types";
import { countReports, countsText, parseOrigins } from "./model";

const report = (state: ReportState) => ({ state }) as Report;

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
    expect(c).toEqual({ open: 2, claimed: 1, resolved: 1, more: false });
    expect(countsText(c)).toBe("2 open · 1 claimed · 1 resolved");
  });

  it("stops at the page limit and says there are more", async () => {
    let n = 0;
    const c = await countReports(async () => { n++; return { reports: [report("open")], next: "again" }; }, 3);
    expect(n).toBe(3);
    expect(c).toEqual({ open: 3, claimed: 0, resolved: 0, more: true });
    expect(countsText(c)).toBe("3+ open · 0+ claimed · 0+ resolved");
  });
});
