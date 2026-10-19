import { describe, expect, it } from "vitest";
import { item } from "./fixtures";
import { type Group, NO_ARTIFACT, appendGroups, appendRows, fromWire, groupHref, groupKey, groupTitle, lists, shortAge, takeItem } from "./inbox-groups";
import { itemTitle } from "./inbox-model";

const R = item("reply", { id: "IR", seq: 8 });
const S = item("reply", { id: "IS", seq: 7 });
const C = item("connection", { id: "IC", seq: 6 });
const board = (over: Partial<Group> = {}): Group => ({ key: "7q3k9mzx2b4t", artifact: R.artifact, total: 5, unread: 3, newestAt: R.created_at, items: [R, S], next: "7", ...over });

describe("inbox groups", () => {
  it("keys an item by its artifact, and items about none together", () => {
    expect([groupKey(R), groupKey(C)]).toEqual(["7q3k9mzx2b4t", NO_ARTIFACT]);
    const g = fromWire({ artifact_id: null, artifact: null, total: 2, unread: 1, newest_at: C.created_at, items: [C], next_cursor: null });
    expect([g.key, groupTitle(g), groupHref(g)]).toEqual([NO_ARTIFACT, "Other", null]);
    // A page's group whose first item names no artifact keeps the page's key.
    const hidden = fromWire({ artifact_id: "7q3k9mzx2b4t", artifact: null, total: 1, unread: 1, newest_at: R.created_at, items: [R], next_cursor: null });
    expect([hidden.key, groupTitle(hidden), groupHref(hidden)]).toEqual(["7q3k9mzx2b4t", "a deleted page", null]);
  });

  it("titles a group by its page and links it while the page has a title", () => {
    expect([groupTitle(board()), groupHref(board())]).toEqual(["Quarterly Review", "/a/7q3k9mzx2b4t"]);
    const gone = board({ artifact: { id: "7q3k9mzx2b4t", title: null, kind: null, page_url: null } });
    expect([groupTitle(gone), groupHref(gone)]).toEqual(["a deleted page", null]);
  });

  it("says a short age", () => {
    const now = new Date("2026-10-07T12:00:00.000Z");
    expect(["2026-10-07T11:59:30.000Z", "2026-10-07T11:58:00.000Z", "2026-10-07T09:00:00.000Z", "2026-10-04T12:00:00.000Z", "2026-09-01T12:00:00.000Z"].map(t => shortAge(t, now)))
      .toEqual(["now", "2m", "3h", "3d", "2026-09-01"]);
  });

  it("appends a group's rows and another page of groups without repeats", () => {
    const T = item("reply", { id: "IT", seq: 5 });
    const [g] = appendRows([board()], "7q3k9mzx2b4t", [S, T], null);
    expect([g.items.map(i => i.id), g.next]).toEqual([["IR", "IS", "IT"], null]);
    const other = board({ key: NO_ARTIFACT, artifact: null, items: [C] });
    expect(appendGroups([board()], [board({ items: [] }), other]).map(x => [x.key, x.items.length])).toEqual([["7q3k9mzx2b4t", 2], [NO_ARTIFACT, 1]]);
  });

  it("takes an item as it is now: replaced where listed, its group's unread count moved", () => {
    const read = { ...S, read: true };
    const [g] = takeItem([board()], read, -1);
    expect([g.items[1].read, g.unread, g.total]).toEqual([true, 2, 5]);
    // Counted even when its group does not list it (a card, or past the rows shown).
    const [h] = takeItem([board()], { ...item("reply", { id: "IX", seq: 2 }), read: true }, -1);
    expect([h.items.map(i => i.id), h.unread]).toEqual([["IR", "IS"], 2]);
    const gs = [board()];
    expect(takeItem(gs, C, 0)[0]).toBe(gs[0]);
    expect([lists(gs, "IS"), lists(gs, "IC"), lists(undefined, "IS")]).toEqual([true, false, false]);
  });

  it("leaves the page out of a grouped item's title", () => {
    expect([itemTitle(R, [], { page: false }), itemTitle(item("version"), [], { page: false }), itemTitle(item("published"), [], { page: false }), itemTitle(item("finished"), [], { page: false })])
      .toEqual(["claude replied", "claude published v4 · addressed 2 of your threads", "claude published the page", "claude finished"]);
    expect(itemTitle(R)).toBe("claude replied on Quarterly Review");
  });
});
