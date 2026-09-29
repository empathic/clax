import { describe, expect, it } from "vitest";
import { MAX_CLIP_REGION, MAX_SIDE, REGION_PAD, blockAncestor, clipBackground, clipRootStyle, clipScale, crossOriginImage, dataUrlToBuffer, fitsClipBudget, elementRegion, regionBounds } from "../src/clip";

describe("clip helpers", () => {
  it("renders at device pixel ratio but never past 1600 px on the long side", () => {
    expect(clipScale(400, 300, 2)).toBe(2);
    expect(clipScale(1200, 300, 2)).toBeCloseTo(MAX_SIDE / 1200);
    expect(clipScale(3200, 10, 1)).toBe(0.5);
    expect(clipScale(0, 0, 3)).toBe(3);
  });
  it("walks from a text node to the nearest block ancestor", () => {
    document.body.innerHTML = `<div id="d"><p id="p">a <b><i>word</i></b> b</p></div>`;
    const text = document.querySelector("i")!.firstChild!;
    expect(blockAncestor(text, window).id).toBe("p");
  });
  it("flags only images from other origins", () => {
    const img = document.createElement("img");
    img.src = "https://cdn.example/x.png";
    expect(crossOriginImage(img, "http://localhost:7480")).toBe(true);
    img.src = "http://localhost:7480/_blob/1";
    expect(crossOriginImage(img, "http://localhost:7480")).toBe(false);
    img.src = "data:image/png;base64,AAAA";
    expect(crossOriginImage(img, "http://localhost:7480")).toBe(false);
    expect(crossOriginImage(document.createElement("p"), "http://localhost:7480")).toBe(false);
  });
  it("decodes data URLs", () => {
    expect(new Uint8Array(dataUrlToBuffer("data:image/png;base64,iVBORw=="))).toEqual(new Uint8Array([0x89, 0x50, 0x4e, 0x47]));
  });
  it("paints behind the clip the nearest opaque background up the tree", () => {
    document.documentElement.removeAttribute("style");
    document.body.innerHTML = `<div id="d" style="background-color: rgb(10, 20, 30)"><p id="p"><b>x</b></p></div><p id="bare">y</p>`;
    expect(clipBackground(document.querySelector("b")!, window)).toBe("rgb(10, 20, 30)");
    document.body.style.backgroundColor = "rgb(1, 2, 3)";
    expect(clipBackground(document.getElementById("bare")!, window)).toBe("rgb(1, 2, 3)");
    document.body.removeAttribute("style");
  });
  it("falls back to the page canvas colour for its color-scheme", () => {
    document.body.innerHTML = `<p id="bare">y</p>`;
    expect(clipBackground(document.getElementById("bare")!, window)).toBe("#ffffff");
    document.documentElement.style.colorScheme = "dark";
    expect(clipBackground(document.getElementById("bare")!, window)).toBe("#121212");
    document.documentElement.removeAttribute("style");
  });
  it("renders the root without its margins", () => {
    document.body.innerHTML = `<h3 style="margin: 18px 0">x</h3>`;
    expect(clipRootStyle(document.querySelector("h3")!, window)).toEqual({ margin: "0" });
  });
  it("cancels child top margins that collapse through the root", () => {
    document.body.innerHTML = `<main id="m"><section style="margin-top: 4px"><h3 style="margin-top: 18px">x</h3></section><p>y</p></main>`;
    expect(clipRootStyle(document.getElementById("m")!, window)).toEqual({ margin: "0", display: "flow-root", marginTop: "-18px" });
    document.body.innerHTML = `<main id="m" style="padding-top: 1px"><h3 style="margin-top: 18px">x</h3></main>`;
    expect(clipRootStyle(document.getElementById("m")!, window)).toEqual({ margin: "0" });
    document.body.innerHTML = `<main id="m">text <h3 style="margin-top: 18px">x</h3></main>`;
    expect(clipRootStyle(document.getElementById("m")!, window)).toEqual({ margin: "0" });
    document.body.innerHTML = `<main id="m"><span style="margin-top: 18px">x</span></main>`;
    expect(clipRootStyle(document.getElementById("m")!, window)).toEqual({ margin: "0" });
    document.body.innerHTML = `<main id="m" style="display: flex"><h3 style="margin-top: 18px">x</h3></main>`;
    expect(clipRootStyle(document.getElementById("m")!, window)).toEqual({ margin: "0" });
  });

  it("captures any region within the budget, and only those", () => {
    expect(MAX_CLIP_REGION).toEqual({ w: 1600, h: 2400 });
    expect(fitsClipBudget({ width: 900, height: 820 }), "a 40-line code block").toBe(true);
    expect(fitsClipBudget({ width: 1600, height: 2400 })).toBe(true);
    expect(fitsClipBudget({ width: 900, height: 21075 })).toBe(false);
    expect(fitsClipBudget({ width: 1700, height: 100 })).toBe(false);
  });
  it("frames a range in a larger block with the padding above and below, inside the block", () => {
    const block = { top: -2630, bottom: 18445, width: 900, height: 21075 };
    expect(REGION_PAD).toBe(120);
    expect(regionBounds({ top: 300, bottom: 320, width: 200, height: 20 }, block)).toEqual({ top: 180, bottom: 440 });
    // At the block's top and bottom edges.
    expect(regionBounds({ top: -2620, bottom: -2600, width: 200, height: 20 }, block)).toEqual({ top: -2630, bottom: -2480 });
    expect(regionBounds({ top: 18400, bottom: 18440, width: 200, height: 40 }, block)).toEqual({ top: 18280, bottom: 18445 });
    // A range taller than the budget is cut at the budget's height.
    expect(regionBounds({ top: 0, bottom: 5000, width: 200, height: 5000 }, block)).toEqual({ top: -120, bottom: -120 + MAX_CLIP_REGION.h });
  });
  it("crops an element larger than the budget to its part in view, grown to the budget within the element", () => {
    const vp = { w: 1000, h: 800 };
    const el = (left: number, top: number, width: number, height: number) => ({ left, top, width, height });
    // 5,000 px tall, scrolled 2,000 px into it: the 800 px in view plus 800 px above and below.
    expect(elementRegion(el(0, -2000, 900, 5000), vp)).toEqual({ x: 0, y: 1200, w: 900, h: 2400 });
    // Its top in view: the growth goes below.
    expect(elementRegion(el(0, 100, 900, 5000), vp)).toEqual({ x: 0, y: 0, w: 900, h: 2400 });
    // Its bottom in view: the growth goes above.
    expect(elementRegion(el(0, -4500, 900, 5000), vp)).toEqual({ x: 0, y: 2600, w: 900, h: 2400 });
    // Wider than the budget: the same on both axes.
    expect(elementRegion(el(-1000, 0, 4000, 300), vp)).toEqual({ x: 700, y: 0, w: 1600, h: 300 });
    // Out of view (a scroll before the pick): grown from the nearest edge.
    expect(elementRegion(el(0, 900, 900, 5000), vp)).toEqual({ x: 0, y: 0, w: 900, h: 2400 });
    expect(elementRegion(el(0, -6000, 900, 5000), vp)).toEqual({ x: 0, y: 2600, w: 900, h: 2400 });
  });
});
