import { afterEach, describe, expect, it, vi } from "vitest";
import type { ViewState } from "./artifact-controller";
import { setTitleTags, tagsOf } from "./title-tags";

const q4 = { id: "t_aaaaaaaaaa", name: "Q4 plan", colour: "blue" };
const design = { id: "t_bbbbbbbbbb", name: "design", colour: "purple" };

afterEach(() => { document.body.replaceChildren(); });

describe("the artifact's tags beside its title", () => {
  const ttl = () => {
    const el = document.createElement("div");
    el.className = "ttl";
    el.innerHTML = "<h1>Plan</h1><span class=\"by\"></span>";
    document.body.append(el);
    return el;
  };

  it("links each tag to the gallery filtered by it, and offers Edit tags only to a viewer who may set them", () => {
    const el = ttl();
    const edit = vi.fn();
    setTitleTags(el, [design, q4], false, edit);
    const links = [...el.querySelectorAll<HTMLAnchorElement>(".ttags a.tag")];
    expect(links.map(a => [a.textContent, a.getAttribute("href"), a.dataset.c])).toEqual([["design", "/?tag=design", "purple"], ["Q4 plan", "/?tag=Q4%20plan", "blue"]]);
    expect(el.querySelector(".tedit")).toBeNull();
    setTitleTags(el, [design], true, edit);
    const b = el.querySelector<HTMLButtonElement>(".ttags .tedit")!;
    expect([b.textContent, b.getAttribute("aria-haspopup"), b.getAttribute("aria-expanded")]).toEqual(["Edit tags", "dialog", "false"]);
    b.click();
    expect(edit).toHaveBeenCalledWith(b);
    // The same button stays as the tags change, so an open editor keeps its place.
    setTitleTags(el, [], true, edit);
    expect(el.querySelector(".tedit")).toBe(b);
    expect(b.textContent).toBe("Add tags");
    expect(el.querySelectorAll("a.tag")).toHaveLength(0);
    // No tags and no right to set them: nothing.
    setTitleTags(el, [], false, edit);
    expect(el.querySelector(".ttags")).toBeNull();
  });

  it("leaves the DOM alone when nothing changed", () => {
    const el = ttl();
    setTitleTags(el, [q4], true, () => {});
    const chip = el.querySelector("a.tag");
    setTitleTags(el, [{ ...q4 }], true, () => {});
    expect(el.querySelector("a.tag")).toBe(chip);
  });

  it("reads the tags as last refetched, else as loaded", () => {
    const loaded = { data: { artifact: { tags: [q4], may_tag: true } }, tags: null } as unknown as ViewState;
    expect(tagsOf(loaded)).toEqual({ tags: [q4], may: true });
    expect(tagsOf({ ...loaded, tags: { tags: [design], may: false } })).toEqual({ tags: [design], may: false });
    expect(tagsOf({ data: null, tags: null } as unknown as ViewState)).toEqual({ tags: [], may: false });
  });
});
