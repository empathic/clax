// Draws the Echo mark (two half-discs facing an ink dot; red-orange people
// on the left, green agents on the right) as the extension's PNG icons, so
// no binary image lives in the repository. 4×4 supersampling per pixel.
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
// The mark in its 30×24 viewBox (web/shell/src/ui/Mark.svelte).
const PEOPLE = [0xe0, 0x53, 0x2f], AGENTS = [0x2f, 0x6f, 0x2a], INK = [0x1c, 0x1b, 0x19];
function colourAt(x, y) {
  const d = (cx, cy) => Math.hypot(x - cx, y - cy);
  if (x >= 2 && Math.abs(d(2, 12) - 9.5) <= 2.1) return PEOPLE;
  if (x <= 28 && Math.abs(d(28, 12) - 9.5) <= 2.1) return AGENTS;
  if (d(15, 12) <= 2.6) return INK;
  return null;
}
export function drawIcons(dir) {
  mkdirSync(dir, { recursive: true });
  for (const size of [16, 32, 48, 128]) {
    const rgba = Buffer.alloc(size * size * 4);
    const scale = 30 / size;
    for (let py = 0; py < size; py++) for (let px = 0; px < size; px++) {
      const sum = [0, 0, 0, 0];
      for (let sy = 0; sy < 4; sy++) for (let sx = 0; sx < 4; sx++) {
        const c = colourAt((px + (sx + 0.5) / 4) * scale, (py + (sy + 0.5) / 4) * scale - 3);
        if (c) { sum[0] += c[0]; sum[1] += c[1]; sum[2] += c[2]; sum[3] += 1; }
      }
      const i = (py * size + px) * 4;
      if (sum[3]) { rgba[i] = sum[0] / sum[3]; rgba[i + 1] = sum[1] / sum[3]; rgba[i + 2] = sum[2] / sum[3]; rgba[i + 3] = Math.round((sum[3] / 16) * 255); }
    }
    writeFileSync(`${dir}/${size}.png`, png(size, rgba));
  }
}
