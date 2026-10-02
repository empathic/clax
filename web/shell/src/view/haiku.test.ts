import { describe, expect, it } from "vitest";
import list from "./haiku.json";
import { pickHaiku } from "./haiku";

describe("haiku", () => {
  it("are ten, three lines each, and a seed picks the same one every time", () => {
    expect(list).toHaveLength(10);
    for (const h of list) expect(h.split("\n")).toHaveLength(3);
    expect(pickHaiku(list, "01J9ABC")).toBe(pickHaiku(list, "01J9ABC"));
    expect(list).toContain(pickHaiku(list));
  });
});
