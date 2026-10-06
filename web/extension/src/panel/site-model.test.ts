import { describe, expect, it } from "vitest";
import type { Thread } from "../../../shell/src/threads";
import type { PageView, SiteView } from "../messages";
import { checkPattern, counts, groups, lastActivity, matchPattern, matches, preview, statusOf } from "./site-model";

const O = "http://localhost:5173";
const page = (aid: string, path: string, extra: Partial<PageView> = {}): PageView => ({ artifact_id: aid, origin: O, path, page_url: O + path, title: path, current_version: 1, url: `http://127.0.0.1:7481/a/${aid}`, ...extra });
let n = 0;
function thread(over: Partial<Thread> & { body?: string; author?: string; at?: string } = {}): Thread {
  const id = `01J9${String(++n).padStart(22, "0")}`;
  const { body = "Too wide", author = "Alex", at = "2026-10-05T10:00:00.000Z", ...rest } = over;
  return {
    id, artifact_id: "a", version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: at, resolved_at: null, resolved_by: null,
    anchor: { kind: "element", selector: "h1", quote: "Settings", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
    comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: author, via_harness: null, body, created_at: at }], feedback_state: null, addressed_in: [],
    ...rest,
  } as Thread;
}

describe("site model", () => {
  it("tells a thread's status as the daemon counts it", () => {
    expect(statusOf(thread())).toBe("open");
    expect(statusOf(thread({ addressed_in: [3] }))).toBe("addressed");
    expect(statusOf(thread({ addressed_pending: { harness: "claude", at: "t" } }))).toBe("addressed");
    expect(statusOf(thread({ status: "resolved", addressed_in: [3] }))).toBe("resolved");
    expect(counts([thread(), thread({ addressed_in: [1] }), thread({ status: "resolved" }), thread()])).toEqual({ open: 2, addressed: 1, resolved: 1 });
  });

  it("finds a thread's newest activity in its comments and its resolve", () => {
    const t = thread({ at: "2026-10-05T10:00:00.000Z" });
    expect(lastActivity(t)).toBe("2026-10-05T10:00:00.000Z");
    expect(lastActivity({ ...t, comments: [...t.comments, { ...t.comments[0], id: "c2", created_at: "2026-10-05T11:00:00.000Z" }] })).toBe("2026-10-05T11:00:00.000Z");
    expect(lastActivity({ ...t, resolved_at: "2026-10-06T09:00:00.000Z" })).toBe("2026-10-06T09:00:00.000Z");
  });

  it("filters by status and searches text, author, quote and page path, every word", () => {
    const t = thread({ body: "The Save button is cramped", author: "Mia", page_path: "/users/7" });
    expect(matches(t, "all", "")).toBe(true);
    expect(matches(t, "resolved", "")).toBe(false);
    for (const q of ["save", "MIA", "users/7", "settings", "save mia"]) expect(matches(t, "open", q), q).toBe(true);
    for (const q of ["billing", "save billing"]) expect(matches(t, "all", q), q).toBe(false);
  });

  it("groups the other pages' threads by page, newest activity first, a merged page under its pattern", () => {
    const site: SiteView = { origin: O, rules: [], pages: [
      { page: page("aaaaaaaaaaaa", "/"), threads: [thread({ at: "2026-10-05T12:00:00.000Z" })] },
      { page: page("bbbbbbbbbbbb", "/old"), threads: [thread({ at: "2026-10-01T10:00:00.000Z", status: "resolved" })] },
      { page: page("cccccccccccc", "/users/:id", { merged: true, pattern: "/users/:id" }), threads: [
        thread({ at: "2026-10-04T10:00:00.000Z", page_path: "/users/1" }), thread({ at: "2026-10-05T09:00:00.000Z", page_path: "/users/2", addressed_in: [2] }),
      ] },
    ] };
    const all = groups(site, "aaaaaaaaaaaa", "all", "");
    expect(all.map(g => g.label)).toEqual(["/users/:id", "/old"]);
    expect(all[0].counts).toEqual({ open: 1, addressed: 1, resolved: 0 });
    expect(all[0].threads.map(t => t.page_path)).toEqual(["/users/2", "/users/1"]);
    // A filter or a search leaves out the pages with nothing that passes; the counts stay the page's.
    const open = groups(site, "aaaaaaaaaaaa", "open", "");
    expect(open.map(g => [g.label, g.threads.length, g.counts.addressed])).toEqual([["/users/:id", 1, 1]]);
    expect(groups(site, "aaaaaaaaaaaa", "all", "users/2").map(g => g.threads.length)).toEqual([1]);
    expect(groups(site, null, "all", "").map(g => g.label)).toEqual(["/", "/users/:id", "/old"]);
    expect(groups(null, null, "all", "")).toEqual([]);
  });

  it("checks a pattern as the daemon does", () => {
    for (const ok of ["/users/:id", "/docs/*", "/a/:b/c", "/users/:id/edit", "/~me/:x", "/p%20q/:x"]) expect(checkPattern(ok), ok).toBeNull();
    expect(checkPattern("users/:id")).toMatch(/starts with/);
    expect(checkPattern("/*")).toMatch(/fixed segment/);
    expect(checkPattern("/:x")).toMatch(/fixed segment/);
    expect(checkPattern("/:a/:b")).toMatch(/fixed segment/);
    expect(checkPattern("/users")).toMatch(/:name/);
    expect(checkPattern("/users/")).toMatch(/empty segment/);
    expect(checkPattern("/a//:b")).toMatch(/empty segment/);
    expect(checkPattern("/a/*/b")).toMatch(/last/);
    expect(checkPattern("/a/:")).toMatch(/not a name/);
    expect(checkPattern(`/a/:${"x".repeat(33)}`)).toMatch(/not a name/);
    expect(checkPattern("/../:x")).toMatch(/not a path segment/);
    expect(checkPattern("/a b/:x")).toMatch(/not a path segment/);
    expect(checkPattern("/a%zz/:x")).toMatch(/not a path segment/);
    expect(checkPattern(`/${"a/".repeat(16)}:x`)).toMatch(/16 segments/);
    expect(checkPattern(`/${"a".repeat(256)}/:x`)).toMatch(/256 bytes/);
  });

  it("matches paths as the daemon does, and previews the pages a rule would merge", () => {
    expect(matchPattern("/users/:id", "/users/7")).toBe(true);
    expect(matchPattern("/users/:id", "/users/7/")).toBe(false);
    expect(matchPattern("/users/:id", "/users/7/edit")).toBe(false);
    expect(matchPattern("/users/:id", "/users")).toBe(false);
    expect(matchPattern("/users/:id", "/users/")).toBe(false);
    expect(matchPattern("/docs/*", "/docs/a/b")).toBe(true);
    expect(matchPattern("/docs/*", "/docs")).toBe(false);
    expect(matchPattern("/docs/*", "/docs/")).toBe(false);
    const site: SiteView = { origin: O, rules: [], pages: [
      { page: page("aaaaaaaaaaaa", "/users/1"), threads: [] }, { page: page("bbbbbbbbbbbb", "/users/2"), threads: [] },
      { page: page("cccccccccccc", "/users"), threads: [] }, { page: page("dddddddddddd", "/users/:id", { merged: true, pattern: "/users/:id" }), threads: [] },
    ] };
    expect(preview(site, "/users/:id")).toEqual(["/users/1", "/users/2"]);
    expect(preview(site, "/:x")).toEqual([]);
  });
});
