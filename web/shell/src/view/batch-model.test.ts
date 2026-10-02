import { describe, expect, it } from "vitest";
import type { Thread } from "../threads";
import { EMPTY_SELECTION, countLabel, prune, selectable, sendLabel, toggle, unsent, unsentLabel } from "./batch-model";

const T = (id: string, status: "open" | "resolved" = "open", sent = false) => ({ id, status, sent_to_agent: sent } as Thread);
const order = ["a", "b", "c", "d", "e"];

describe("batch-model", () => {
  it("selects open threads of a live artifact only", () => {
    expect([selectable(T("a"), false), selectable(T("a", "resolved"), false), selectable(T("a"), true)]).toEqual([true, false, false]);
  });
  it("toggles one, then a shift range to the clicked box's new state, in sidebar order", () => {
    let s = toggle(EMPTY_SELECTION, "b", false, order);
    expect(s).toEqual({ ids: ["b"], anchor: "b" });
    s = toggle(s, "d", true, order);
    expect(s.ids).toEqual(["b", "c", "d"]);
    s = toggle(s, "c", false, order);
    expect(s).toEqual({ ids: ["b", "d"], anchor: "c" });
  });
  it("drops threads that disappeared, were resolved, or whose artifact went", () => {
    const s = { ids: ["a", "b", "c"], anchor: "c" };
    expect(prune(s, [T("a"), T("b", "resolved")], false)).toEqual({ ids: ["a"], anchor: null });
    expect(prune(s, [T("a"), T("b"), T("c")], true)).toEqual(EMPTY_SELECTION);
    expect(prune(s, [T("a"), T("b"), T("c")], false)).toBe(s);
  });
  it("counts unsent threads and labels in Echo's words", () => {
    expect(unsent([T("a"), T("b", "open", true), T("c", "resolved")]).map(t => t.id)).toEqual(["a"]);
    expect([countLabel(3), unsentLabel(4, "claude"), sendLabel(3, "claude"), sendLabel(1, "codex")]).toEqual(["3 selected", "Send 4 unsent to claude", "Send 3 to claude", "Send to codex"]);
  });
});
