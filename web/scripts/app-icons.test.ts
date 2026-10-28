import { readFileSync } from "node:fs";
import { join } from "node:path";
import { inflateSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import { APP_BACKGROUND, appIcons } from "./extension-icons.mjs";

/** Width, height and RGBA rows of a PNG drawMark wrote (8-bit RGBA, filter 0, one IDAT). */
function decode(png: Buffer): { w: number; h: number; px: (x: number, y: number) => number[] } {
  expect([...png.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
  expect(png.toString("ascii", 12, 16)).toBe("IHDR");
  const w = png.readUInt32BE(16), h = png.readUInt32BE(20);
  const idat = png.indexOf("IDAT");
  const raw = inflateSync(png.subarray(idat + 4, idat + 4 + png.readUInt32BE(idat - 4)));
  return { w, h, px: (x, y) => [...raw.subarray(y * (w * 4 + 1) + 1 + x * 4, y * (w * 4 + 1) + 5 + x * 4)] };
}

describe("the installed web app's icons", () => {
  it("are the sizes the manifest and the apple-touch-icon name, opaque on the shell's background", () => {
    const icons = appIcons();
    expect(Object.keys(icons).sort()).toEqual(["apple-touch-icon.png", "icon-192.png", "icon-512.png"]);
    for (const [name, size] of [["apple-touch-icon.png", 180], ["icon-192.png", 192], ["icon-512.png", 512]] as const) {
      const { w, h, px } = decode(icons[name]);
      expect([w, h]).toEqual([size, size]);
      // Corners are the background, fully opaque (iOS would fill transparency with black).
      for (const [x, y] of [[0, 0], [size - 1, 0], [0, size - 1], [size - 1, size - 1]]) expect(px(x, y)).toEqual([...APP_BACKGROUND, 255]);
      // The ink dot sits at the centre.
      const c = px(size / 2, size / 2);
      expect(c[3]).toBe(255);
      expect(c[0]).toBeLessThan(0x40);
    }
  });

  it("are the ones the manifest lists", () => {
    const manifest = JSON.parse(readFileSync(join(__dirname, "../shell/public/_clax/app.webmanifest"), "utf8"));
    expect(manifest).toMatchObject({ name: "Clax", start_url: "/", scope: "/", display: "standalone", theme_color: "#f7f7f5", background_color: "#f7f7f5" });
    const names = Object.keys(appIcons()).map(n => `/_clax/app/${n}`);
    for (const i of manifest.icons) expect(names).toContain(i.src);
    for (const size of ["192x192", "512x512"]) expect(manifest.icons.some((i: { sizes: string }) => i.sizes === size)).toBe(true);
    for (const html of ["index.html", "artifact.html"]) {
      const text = readFileSync(join(__dirname, `../shell/${html}`), "utf8");
      expect(text).toContain(`<link rel="manifest" href="/_clax/app.webmanifest">`);
      expect(text).toContain(`<link rel="apple-touch-icon" href="/_clax/app/apple-touch-icon.png">`);
    }
  });
});
