import { describe, expect, it } from "vitest";
import type { TagRef } from "../api";
import { filterArtifacts } from "./gallery-model";
import { barTags, canonical, carriesAll, holds, names, refsForUrl, same, tagsFromUrl, tagsInUse, toggle, withTags } from "./tags-model";

const q4: TagRef = { id: "t_aaaaaaaaaa", name: "Q4", colour: "blue" };
const design: TagRef = { id: "t_bbbbbbbbbb", name: "design", colour: "purple" };
const art = (title: string, tags: TagRef[] = [], description: string | null = null) =>
  ({ id: title, title, description, icon: null, updated_at: "2026-10-01T00:00:00Z", current_version: 1, pinned: false, tags });

describe("tags in the address", () => {
  it("reads every tag parameter, trimmed, without blanks or repeats in any case", () => {
    expect(tagsFromUrl("?tag=q4&x=1&tag=%20design%20&tag=&tag=Q4&tag=a+b")).toEqual(["q4", "design", "a b"]);
    expect(tagsFromUrl("")).toEqual([]);
  });

  it("writes them back after the other parameters, and leaves none for an empty filter", () => {
    expect(withTags("?q=1&tag=old", ["Q4", "a b"])).toBe("?q=1&tag=Q4&tag=a+b");
    expect(withTags("?tag=old", [])).toBe("");
    expect(tagsFromUrl(withTags("", ["a&b", "c=d"]))).toEqual(["a&b", "c=d"]);
  });
});

describe("filtering by tags", () => {
  it("names a tag by its ID or its name in any case", () => {
    expect(names(q4, "q4")).toBe(true);
    expect(names(q4, " Q4 ")).toBe(true);
    expect(names(q4, q4.id)).toBe(true);
    expect(names(q4, "design")).toBe(false);
  });

  it("keeps the artifacts carrying every tag named, and every artifact for none", () => {
    const both = art("both", [design, q4]);
    const one = art("one", [q4]);
    const none = art("none");
    expect([both, one, none].filter(a => carriesAll(a, ["q4"]))).toEqual([both, one]);
    expect([both, one, none].filter(a => carriesAll(a, ["q4", "DESIGN"]))).toEqual([both]);
    expect([both, one, none].filter(a => carriesAll(a, []))).toHaveLength(3);
    expect(carriesAll(both, ["nope"])).toBe(false);
  });

  it("toggles a tag in by its ID and out however the filter names it", () => {
    expect(toggle([], q4)).toEqual([q4.id]);
    expect(toggle(["q4", "design"], q4)).toEqual(["design"]);
    // A filter naming a tag by its ID lets go of it at the first press.
    expect(toggle([q4.id], q4)).toEqual([]);
    expect(toggle([q4.id, "Q4"], q4)).toEqual([]);
    expect(holds(["design"], design)).toBe(true);
    expect(holds([design.id], design)).toBe(true);
    expect(holds(["q4"], design)).toBe(false);
  });

  it("holds a filter by tag ID, names it in the address, follows a rename and drops a deleted tag", () => {
    // Names and IDs from an address become IDs, once each; a name no tag has stays.
    expect(canonical(["q4", design.id, "Q4", "nope"], [q4, design])).toEqual([q4.id, design.id, "nope"]);
    // A tag no card carries is found in the whole list.
    expect(canonical(["design"], [q4], [q4, design])).toEqual([design.id]);
    // Deleted: the whole list lacks its ID, and it drops out; without the list, it stays.
    expect(canonical([q4.id, design.id], [q4], [q4])).toEqual([q4.id]);
    expect(canonical([design.id], [q4])).toEqual([design.id]);
    // The address names each tag, under its name now.
    const renamed = { ...q4, name: "Q4 plan" };
    expect(refsForUrl([q4.id, "nope"], [renamed])).toEqual(["Q4 plan", "nope"]);
    expect(tagsFromUrl(withTags("", refsForUrl([q4.id], [renamed])))).toEqual(["Q4 plan"]);
    expect(same(["a"], ["a"])).toBe(true);
    expect(same(["a"], ["a", "b"])).toBe(false);
    // The bar shows a filtered tag no card carries in its own colour.
    expect(barTags([q4], [q4.id, design.id], [q4, design])).toEqual([q4, design]);
  });

  it("lists the tags in use once each, by name, then the filter's tags no artifact carries", () => {
    const inUse = tagsInUse([art("a", [q4]), art("b", [design, q4]), art("c")]);
    expect(inUse.map(t => t.name)).toEqual(["design", "Q4"]);
    expect(barTags(inUse, ["q4", "gone"]).map(t => [t.name, t.colour])).toEqual([["design", "purple"], ["Q4", "blue"], ["gone", "gray"]]);
  });

  it("matches a search against tag names as well as titles and descriptions", () => {
    const tagged = art("Roadmap", [design]);
    const titled = art("Design notes");
    const plain = art("Other", [], "nothing here");
    expect(filterArtifacts([tagged, titled, plain], "desig")).toEqual([tagged, titled]);
    expect(filterArtifacts([tagged, titled, plain], "  ")).toHaveLength(3);
  });
});
