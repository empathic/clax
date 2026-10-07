import { describe, expect, it } from "vitest";
import { item, view } from "./fixtures";
import { filterFromUrl, filterToUrl, itemText, itemTitle, notificationText } from "./inbox-model";

describe("inbox model", () => {
  it("titles each kind", () => {
    expect(itemTitle(item("reply"))).toBe("claude replied on Quarterly Review");
    expect(itemTitle(item("version"))).toBe("claude published v4 of Quarterly Review · addressed 2 of your threads");
    expect(itemTitle(item("version", { version: { n: 5, note: null, addressed: [{ id: "t1", summary: "h2" }] } }))).toBe("claude published v5 of Quarterly Review · addressed 1 of your threads");
    expect(itemTitle(item("version", { version: { n: 6, note: null, addressed: [] } }))).toBe("claude published v6 of Quarterly Review");
    expect(itemTitle(item("published"))).toBe("claude published Sales dashboard");
    expect(itemTitle(item("question"))).toBe("claude asks: Layout, Sidebars, Notes");
    expect(itemTitle(item("finished"))).toBe("claude finished on Quarterly Review");
  });

  it("names a second agent of a harness, and a deleted page", () => {
    const other = item("reply", { agent: { handle: "a_9c2b00", harness: "claude", project: "p" } });
    expect(itemTitle(item("reply"), [item("reply"), other])).toBe("claude 1f3a replied on Quarterly Review");
    expect(itemTitle(item("finished", { gone: true, artifact: { id: "7q3k9mzx2b4t", title: null, kind: null, page_url: null } }))).toBe("claude finished on a deleted page");
  });

  it("puts each kind's text on one line", () => {
    expect(itemText(item("reply"))).toBe("Done: two columns now. The table sorts too.");
    expect(itemText(item("version"))).toBe("Two columns, sortable table");
    expect(itemText(item("published"))).toBe("Weekly sales by region");
    expect(itemText(item("question"))).toBe("Which layout should the dashboard use?");
    const answered = view({ status: "answered", answers: [{ selected: ["Two"], text: null }, { selected: [], text: null }, { selected: [], text: "keep it" }] });
    expect(itemText(item("question", { question: answered }))).toBe("Layout: Two · Sidebars: — · Notes: keep it");
    expect(itemText(item("finished"))).toBe("Charts are in.");
    expect(itemText(item("reply", { gone: true, reply: { comment_id: "c", body: null, addressed: false } }))).toBe("");
  });

  it("keeps the search in the URL and reads it back", () => {
    const s = "?search=dash&kind=reply,question&agent=claude&since=2026-10-01";
    const f = filterFromUrl(s);
    expect(f).toEqual({ q: "dash", kind: ["reply", "question"], agent: "claude", since: "2026-10-01" });
    expect(filterToUrl(f)).toBe(s);
    expect(filterFromUrl("?kind=reply,bogus&search=%20&until=2026-10-07")).toEqual({ kind: ["reply"], until: "2026-10-07" });
    expect(filterToUrl({})).toBe("");
    expect(filterFromUrl(filterToUrl({ q: "a&b=c #d" }))).toEqual({ q: "a&b=c #d" });
  });

  it("strips control and bidirectional characters from a notification and cuts its body at 180", () => {
    const n = notificationText(item("reply", { reply: { comment_id: "c", body: "\u202eevil\u0007 done\u200f\u2066", addressed: false } }));
    expect(n).toEqual({ title: "claude replied on Quarterly Review", body: "evil done" });
    const long = notificationText(item("reply", { reply: { comment_id: "c", body: "y".repeat(400), addressed: false } }));
    expect([...long.body]).toHaveLength(180);
    expect(long.body.endsWith("y…")).toBe(true);
    const t = notificationText(item("published", { artifact: { id: "x", title: "Sales\u202e\u0000 board", kind: "html", page_url: null } }));
    expect(t.title).toBe("claude published Sales board");
    expect(notificationText(item("published", { published: { description: "y".repeat(180) } })).body).toBe("y".repeat(180));
  });
});
