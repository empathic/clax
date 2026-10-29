import { describe, expect, it } from "vitest";
import type { Report, ReportPage, ReportState } from "../../../sdk/src/types";
import { anchorText, browserOf, commitLink, countReports, countsText, parseOrigins, queuesRoute, reportHref, reporterName } from "./model";

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
      c2: { reports: [report("resolved"), report("fix_proposed"), report("verified")], next: null },
    };
    const asked: (string | null)[] = [];
    const c = await countReports(async cursor => { asked.push(cursor); return pages[cursor ?? ""]; });
    expect(asked).toEqual([null, "c2"]);
    expect(c).toEqual({ open: 2, claimed: 1, fix_proposed: 1, verified: 1, resolved: 1, more: false });
    expect(countsText(c)).toBe("2 open · 1 claimed · 1 fix proposed · 1 verified · 1 resolved");
  });

  it("stops at the page limit and says there are more", async () => {
    let n = 0;
    const c = await countReports(async () => { n++; return { reports: [report("open")], next: "again" }; }, 3);
    expect(n).toBe(3);
    expect(c).toEqual({ open: 3, claimed: 0, fix_proposed: 0, verified: 0, resolved: 0, more: true });
    expect(countsText(c)).toBe("3+ open · 0+ claimed · 0+ fix proposed · 0+ verified · 0+ resolved");
  });
});

describe("the Queues page's addresses", () => {
  it("reads the list, a queue's view and a report's view from the path", () => {
    expect(queuesRoute("/queues")).toEqual({ view: "list" });
    expect(queuesRoute("/queues/q1")).toEqual({ view: "queue", qid: "q1" });
    expect(queuesRoute("/queues/q1/t%2F1")).toEqual({ view: "report", qid: "q1", tid: "t/1" });
    expect(queuesRoute("/queues/q1/t1/x")).toEqual({ view: "list" });
    expect(reportHref("q 1", "t/1")).toBe("/queues/q%201/t%2F1");
  });
});

describe("a report's reporter, commit, browser and anchor", () => {
  it("names the reporter by profile, then reporter ID", () => {
    expect(reporterName({ reporter_profile: { name: "Ana", github: "ana" }, reporter_id: "emp-1" })).toBe("Ana");
    expect(reporterName({ reporter_profile: { github: "ana" }, reporter_id: "emp-1" })).toBe("@ana");
    expect(reporterName({ reporter_id: "emp-1" })).toBe("emp-1");
    expect(reporterName({ reporter_id: null })).toBe("Anonymous");
  });

  it("links the commit when app.repo names the repository, and shows it as text otherwise", () => {
    const ctx = (app: unknown, commit = "f7e862ab0aa1bb2cc3dd") => ({ v: 1 as const, captured_at: "2026-10-29T07:12:03Z", release: { commit }, app });
    expect(commitLink(ctx({ repo: "https://github.com/empathic/clax.git" }))).toEqual({ text: "f7e862ab0aa1", href: "https://github.com/empathic/clax/commit/f7e862ab0aa1bb2cc3dd", host: "github.com" });
    expect(commitLink(ctx({ repo: "empathic/clax" }))!.href).toBe("https://github.com/empathic/clax/commit/f7e862ab0aa1bb2cc3dd");
    expect(commitLink(ctx({ repo: "javascript:alert(1)" }))).toEqual({ text: "f7e862ab0aa1", href: null, host: null });
    expect(commitLink(ctx(undefined))!.href).toBeNull();
    expect(commitLink(ctx({ repo: "empathic/clax" }, "../x?y"))!.href).toBeNull();
    expect(commitLink({ v: 1, captured_at: "x" })).toBeNull();
  });

  it("reads the browser from a user agent", () => {
    expect(browserOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")).toBe("Chrome 131");
    expect(browserOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15")).toBe("Safari 18");
    expect(browserOf("Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0")).toBe("Firefox 132");
    expect(browserOf("Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0")).toBe("Edge 131");
    expect(browserOf("curl/8")).toBe("curl/8");
  });

  it("says what the anchor points at", () => {
    expect(anchorText({ kind: "range", quote: "Total" })).toBe("Text: “Total”");
    expect(anchorText({ kind: "element", name: "Save button", selector: "#save" })).toBe("Element: Save button");
    expect(anchorText({ kind: "artifact" })).toBe("The whole page");
    expect(anchorText(null)).toBeNull();
  });
});
