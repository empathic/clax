import { describe, expect, it } from "vitest";
import { MAX_SIDE, blockAncestor, clipBackground, clipRootStyle, clipScale, crossOriginImage, dataUrlToBuffer } from "../src/clip";

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
});
