import { describe, expect, it } from "vitest";
import { filterLabel, matchTags, step } from "./tag-picker-model";

const tags = [{ id: "1", name: "Design", colour: "blue" }, { id: "2", name: "draft", colour: "gray" }, { id: "3", name: "Q4", colour: "red" }];

describe("the tag picker", () => {
  it("shows the tags whose name holds the search, in any case, in order", () => {
    expect(matchTags(tags, " D ").map(t => t.name)).toEqual(["Design", "draft"]);
    expect(matchTags(tags, "q4").map(t => t.name)).toEqual(["Q4"]);
    expect(matchTags(tags, "")).toEqual(tags);
    expect(matchTags(tags, "zz")).toEqual([]);
  });

  it("moves between the search (-1) and the tags with the arrows, Home and End", () => {
    expect(step(-1, 3, "ArrowDown")).toBe(0);
    expect(step(0, 3, "ArrowDown")).toBe(1);
    expect(step(2, 3, "ArrowDown")).toBe(2);
    expect(step(1, 3, "ArrowUp")).toBe(0);
    expect(step(0, 3, "ArrowUp")).toBe(-1);
    expect(step(-1, 3, "ArrowUp")).toBe(-1);
    expect(step(1, 3, "Home")).toBe(0);
    expect(step(0, 3, "End")).toBe(2);
    // In the search, Home and End are the field's.
    expect(step(-1, 3, "Home")).toBeNull();
    expect(step(1, 3, "a")).toBeNull();
    expect(step(-1, 0, "ArrowDown")).toBeNull();
  });

  it("says what a filter holds on its button", () => {
    expect(filterLabel([])).toBe("Any tag");
    expect(filterLabel(["q4"])).toBe("q4");
    expect(filterLabel(["q4", "design", "x"])).toBe("q4 +2");
  });
});
