// Prints metric overrides for the local faces that stand in for IBM Plex
// Sans Condensed 600 until it loads (font-display: swap), so the swap barely
// moves the top bar. Plex's ascent and descent are 1.025 and 0.275 em.
//
// Each family is one set of metrics: the local names it lists are one face
// (or metric-compatible faces), so one size-adjust fits them all. A face
// this machine lacks is skipped, unless its file is given on the command
// line as `Family=path/to/face.ttf`:
//
//   node scripts/measure-fallback.mjs "Plex Condensed Fallback D=DejaVuSansCondensed-Bold.ttf"
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const woff = readFileSync(new URL("../shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2", import.meta.url)).toString("base64");
const SAMPLE = "Checkout latency, week 39 Comment Threads v5 of 5 Send to claude Resolve Reply Needs your eyes Addressed in v5 0123456789";
const FALLBACKS = {
  "Plex Condensed Fallback": ["Avenir Next Condensed Demi Bold", "AvenirNextCondensed-DemiBold"],
  "Plex Condensed Fallback D": ["DejaVu Sans Condensed Bold", "DejaVuSansCondensed-Bold"],
  // Liberation Sans Narrow is metric-compatible with Arial Narrow.
  "Plex Condensed Fallback N": ["Arial Narrow Bold", "ArialNarrow-Bold", "Liberation Sans Narrow Bold", "LiberationSansNarrow-Bold"],
};
const files = Object.fromEntries(process.argv.slice(2).map(a => {
  const i = a.indexOf("=");
  if (i < 0 || !(a.slice(0, i) in FALLBACKS)) throw new Error(`expected Family=file, with Family one of ${Object.keys(FALLBACKS).join(", ")}: ${a}`);
  return [a.slice(0, i), readFileSync(a.slice(i + 1)).toString("base64")];
}));

const browser = await chromium.launch();
const page = await browser.newPage();
const given = Object.values(files).map((b, i) => `@font-face{font-family:F${i};src:url(data:font/ttf;base64,${b})}`).join("");
await page.setContent(`<style>@font-face{font-family:P;src:url(data:font/woff2;base64,${woff}) format("woff2");font-weight:600}${given}</style>`);
await page.evaluate(n => Promise.all([document.fonts.load("600 100px P"), ...Array.from({ length: n }, (_, i) => document.fonts.load(`400 100px F${i}`))]), Object.keys(files).length);
const width = (family, weight) => page.evaluate(([f, w, s]) => {
  const c = document.createElement("canvas").getContext("2d");
  c.font = `${w} 100px ${f}`;
  return c.measureText(s).width;
}, [family, weight, SAMPLE]);
const plex = await width("P", 600);
const mono = await width("monospace", 400);
const named = Object.keys(files);
for (const [name, locals] of Object.entries(FALLBACKS)) {
  const at = named.indexOf(name);
  const candidates = at >= 0 ? [[`file`, `F${at}`]] : locals.map(l => [l, `"${l}"`]);
  for (const [label, family] of candidates) {
    // A face this machine lacks falls through to monospace: skip it.
    const w = await width(`${family}, monospace`, 400);
    if (Math.abs(w - mono) < 0.5) continue;
    const sa = plex / w;
    const pct = n => `${(n * 100).toFixed(2)}%`;
    console.log(`${name} (${label}): size-adjust: ${pct(sa)}; ascent-override: ${pct(1.025 / sa)}; descent-override: ${pct(0.275 / sa)}; line-gap-override: 0%;`);
    break;
  }
}
await browser.close();
