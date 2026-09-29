import { describe, expect, it } from "vitest";
import { MAX_SIDE, blockAncestor, clipScale, crossOriginImage, dataUrlToBuffer } from "../src/clip";

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
});
