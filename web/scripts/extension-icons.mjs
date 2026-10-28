// Draws the Echo mark (two half-discs facing an ink dot; red-orange people
// on the left, green agents on the right) as PNG icons, so no binary image
// lives in the repository: the extension's (transparent) and the installed
// web app's (opaque, on the shell's background). 4×4 supersampling per
// pixel.
import { mkdirSync, writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const CRC = new Uint32Array(256).map((_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc32 = buf => { let c = 0xffffffff; for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "ascii");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}
function png(size, rgba) {
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) { raw[y * (size * 4 + 1)] = 0; rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4); }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4); ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}
// The mark as web/shell/src/ui/Mark.svelte draws it in its 40×24 viewBox,
// with the favicon's strokes (web/shell/public/_clax/mark.svg), centred in
// a square 42 units wide: two half-rings of radius 9.5 with round caps, and
// the ink dot between them.
const PEOPLE = [0xe0, 0x53, 0x2f], AGENTS = [0x2f, 0x6f, 0x2a], INK = [0x1c, 0x1b, 0x19];
const HALF = 2.3, R = 9.5;
function ring(x, y, cx, opens) {
  const d = Math.hypot(x - cx, y - 12);
  if (opens * (x - cx) >= 0 && Math.abs(d - R) <= HALF) return true;
  return Math.hypot(x - cx, y - 2.5) <= HALF || Math.hypot(x - cx, y - 21.5) <= HALF;
}
function colourAt(x, y) {
  if (ring(x, y, 2, 1)) return PEOPLE;
  if (ring(x, y, 38, -1)) return AGENTS;
  if (Math.hypot(x - 20, y - 12) <= 2.8) return INK;
  return null;
}

/** The light shell's `--bg` (web/shell/src/theme.css), behind the app icons. */
export const APP_BACKGROUND = [0xf7, 0xf7, 0xf5];

/**
 * The mark as a `size`×`size` PNG. `fill` is the part of the side the
 * mark's 42-unit square spans, centred; `background` (RGB) makes every
 * pixel opaque, else the mark stands on transparency.
 */
export function drawMark(size, { fill = 1, background = null } = {}) {
  const rgba = Buffer.alloc(size * size * 4);
  const scale = 42 / (size * fill);
  const off = (size * (1 - fill)) / 2;
  for (let py = 0; py < size; py++) for (let px = 0; px < size; px++) {
    const sum = [0, 0, 0, 0];
    for (let sy = 0; sy < 4; sy++) for (let sx = 0; sx < 4; sx++) {
      const c = colourAt((px - off + (sx + 0.5) / 4) * scale - 1, (py - off + (sy + 0.5) / 4) * scale - 9);
      if (c) { sum[0] += c[0]; sum[1] += c[1]; sum[2] += c[2]; sum[3] += 1; }
    }
    const i = (py * size + px) * 4;
    if (background) {
      // The covered share of the mark's colour over the background.
      const a = sum[3] / 16;
      for (let k = 0; k < 3; k++) rgba[i + k] = Math.round((sum[3] ? (sum[k] / sum[3]) * a : 0) + background[k] * (1 - a));
      rgba[i + 3] = 255;
    } else if (sum[3]) { rgba[i] = sum[0] / sum[3]; rgba[i + 1] = sum[1] / sum[3]; rgba[i + 2] = sum[2] / sum[3]; rgba[i + 3] = Math.round((sum[3] / 16) * 255); }
  }
  return png(size, rgba);
}

export function drawIcons(dir) {
  mkdirSync(dir, { recursive: true });
  for (const size of [16, 32, 48, 128]) writeFileSync(`${dir}/${size}.png`, drawMark(size));
}

/**
 * The installed web app's icons (web/shell/public/_clax/app.webmanifest and
 * the shell's `apple-touch-icon`), by file name: opaque, since iOS fills a
 * transparent home-screen icon with black, and the mark within the middle
 * 60%, inside a maskable icon's safe zone (the middle 80%).
 */
export function appIcons() {
  const icon = size => drawMark(size, { fill: 0.6, background: APP_BACKGROUND });
  return { "apple-touch-icon.png": icon(180), "icon-192.png": icon(192), "icon-512.png": icon(512) };
}
