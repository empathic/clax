import { afterEach, describe, expect, it } from "vitest";
import { brokenImage, faithfulClone } from "../src/inspect-render";

afterEach(() => { document.body.innerHTML = ""; });

/** `el` as the browser reports an image that failed to load (or loaded). */
const loadState = (img: HTMLImageElement, broken: boolean, box = { width: 120, height: 40 }) => {
  Object.defineProperty(img, "complete", { value: true, configurable: true });
  Object.defineProperty(img, "naturalWidth", { value: broken ? 0 : 50, configurable: true });
  img.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, right: box.width, bottom: box.height, ...box, toJSON() {} });
};

describe("faithfulClone", () => {
  it("paints an empty field's placeholder in its placeholder colour, and leaves a filled one", () => {
    document.body.innerHTML = `<input id="a" placeholder="Your name"><input id="b" placeholder="Your name" value="Ada"><textarea placeholder="Notes"></textarea>`;
    const win = { getComputedStyle: (el: Element, pseudo?: string) => ({ color: pseudo === "::placeholder" ? "rgb(117, 117, 117)" : "rgb(0, 0, 0)" }) } as unknown as Window;
    const onClone = faithfulClone(document.body, win);
    const clones = [...document.body.children].map(el => el.cloneNode(true) as HTMLElement);
    clones.forEach(onClone);
    expect(clones.map(c => c.style.color)).toEqual(["rgb(117, 117, 117)", "", "rgb(117, 117, 117)"]);
  });

  it("draws a broken image as an outlined box with its alt text, at its size, and leaves a loaded one", () => {
    document.body.innerHTML = `<img src="missing.png" alt="Logo"><img src="ok.png" alt="fine">`;
    const [gone, ok] = [...document.images];
    loadState(gone, true);
    loadState(ok, false);
    const onClone = faithfulClone(document.body, window);
    const [cg, co] = [gone, ok].map(i => i.cloneNode(true) as HTMLImageElement);
    cg.setAttribute("srcset", "missing@2x.png 2x");
    onClone(cg);
    onClone(co);
    expect(cg.getAttribute("src")).toBe(brokenImage(120, 40, "Logo"));
    expect(cg.hasAttribute("srcset")).toBe(false);
    expect([cg.style.width, cg.style.height]).toEqual(["120px", "40px"]);
    expect(co.getAttribute("src")).toBe("ok.png");
  });

  it("a broken image's glyph is an outline, not a filled box, and escapes the alt text", () => {
    const svg = decodeURIComponent(brokenImage(10, 5, "<b>&\"x\""));
    expect(svg).toMatch(/^data:image\/svg\+xml;charset=utf-8,<svg /);
    expect(svg).toContain('fill="none"');
    expect(svg).not.toContain("<b>");
    expect(svg).toContain('width="16" height="16"');
  });
});
