import { afterEach, describe, expect, it, vi } from "vitest";
import type { ViewState } from "./artifact-controller";
import { fitTitleTags, setTitleTags, tagsOf } from "./title-tags";

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
  const noop = () => {};

  it("links each tag to the gallery where the viewer reaches it, else shows it plain, and lists them all from one button", () => {
    const el = ttl();
    const list = vi.fn();
    setTitleTags(el, [design, q4], false, true, list, noop);
    const links = [...el.querySelectorAll<HTMLAnchorElement>(".ttags a.tag")];
    expect(links.map(a => [a.textContent, a.getAttribute("href"), a.dataset.c])).toEqual([["design", "/?tag=design", "purple"], ["Q4 plan", "/?tag=Q4%20plan", "blue"]]);
    expect(el.querySelector(".tedit")).toBeNull();
    // "+N" on a wide screen, "N tags" on a phone: one button, for every viewer.
    const more = el.querySelector<HTMLButtonElement>(".ttags button.tmore")!;
    expect([more.getAttribute("aria-label"), more.querySelector(".tm-p")!.textContent, more.getAttribute("aria-haspopup")]).toEqual(["All 2 tags", "2 tags", "dialog"]);
    more.click();
    expect(list).toHaveBeenCalledWith(more);
    // A viewer the gallery would send to sign in gets plain chips.
    setTitleTags(el, [design, q4], false, false, list, noop);
    expect(el.querySelectorAll(".ttags a")).toHaveLength(0);
    expect([...el.querySelectorAll(".ttags span.tag")].map(s => s.textContent)).toEqual(["design", "Q4 plan"]);
  });

  it("offers Edit tags only to a viewer who may set them, keeping the button as the tags change", () => {
    const el = ttl();
    const edit = vi.fn();
    setTitleTags(el, [design], true, true, noop, edit);
    const b = el.querySelector<HTMLButtonElement>(".ttags .tedit")!;
    expect([b.querySelector(".te-l")!.textContent, b.querySelector(".te-s")!.textContent, b.getAttribute("aria-haspopup"), b.getAttribute("aria-expanded")]).toEqual(["Edit tags", "Edit", "dialog", "false"]);
    b.click();
    expect(edit).toHaveBeenCalledWith(b);
    setTitleTags(el, [], true, true, noop, edit);
    expect(el.querySelector(".tedit")).toBe(b);
    expect(b.textContent).toBe("Add tags");
    expect(el.querySelector(".tmore")).toBeNull();
    // No tags and no right to set them: nothing.
    setTitleTags(el, [], false, true, noop, edit);
    expect(el.querySelector(".ttags")).toBeNull();
  });

  it("hides the chips that do not fit, last first, and says how many in +N", () => {
    const el = ttl();
    setTitleTags(el, [design, q4, { ...q4, id: "t_cccccccccc", name: "x" }], false, true, noop, noop);
    const box = el.querySelector<HTMLElement>(".ttags")!;
    const chips = box.querySelector<HTMLElement>(".tchips")!;
    // A line 100 px wide; each chip 60 px, until the chips hidden leave room.
    Object.defineProperty(chips, "clientWidth", { get: () => 100 });
    Object.defineProperty(chips, "scrollWidth", { get: () => [...chips.children].filter(c => !(c as HTMLElement).hidden).length * 60 });
    fitTitleTags(box);
    expect([...chips.children].map(c => (c as HTMLElement).hidden)).toEqual([false, true, true]);
    expect(box.querySelector(".tmore")!.getAttribute("data-over")).toBe("2");
    expect(box.querySelector(".tm-d")!.textContent).toBe("+2");
  });

  it("leaves the DOM alone when nothing changed", () => {
    const el = ttl();
    setTitleTags(el, [q4], true, true, noop, noop);
    const chip = el.querySelector("a.tag");
    setTitleTags(el, [{ ...q4 }], true, true, noop, noop);
    expect(el.querySelector("a.tag")).toBe(chip);
  });

  it("reads the tags as last refetched, else as loaded", () => {
    const loaded = { data: { artifact: { tags: [q4], may_tag: true } }, tags: null } as unknown as ViewState;
    expect(tagsOf(loaded)).toEqual({ tags: [q4], may: true });
    expect(tagsOf({ ...loaded, tags: { tags: [design], may: false } })).toEqual({ tags: [design], may: false });
    expect(tagsOf({ data: null, tags: null } as unknown as ViewState)).toEqual({ tags: [], may: false });
  });
});
