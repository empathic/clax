// Prints metric overrides for the local faces that stand in for IBM Plex
// Sans Condensed 600 until it loads (font-display: swap), so the swap barely
// moves the top bar. Plex's ascent and descent are 1.025 and 0.275 em.
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const woff = readFileSync(new URL("../shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2", import.meta.url)).toString("base64");
const SAMPLE = "Checkout latency, week 39 Comment Threads v5 of 5 Send to claude Resolve Reply Needs your eyes Addressed in v5 0123456789";
const FALLBACKS = {
  "Plex Condensed Fallback": ["Avenir Next Condensed Demi Bold", "AvenirNextCondensed-DemiBold", "Helvetica Neue Condensed Bold", "HelveticaNeue-CondensedBold"],
  "Plex Condensed Fallback L": ["DejaVu Sans Condensed Bold", "DejaVuSansCondensed-Bold", "Liberation Sans Narrow Bold", "LiberationSansNarrow-Bold", "Arial Narrow Bold", "ArialNarrow-Bold"],
};
const browser = await chromium.launch();
const page = await browser.newPage();
await page.setContent(`<style>@font-face{font-family:P;src:url(data:font/woff2;base64,${woff}) format("woff2");font-weight:600}</style>`);
await page.evaluate(() => document.fonts.load("600 100px P"));
const width = (family, weight) => page.evaluate(([f, w, s]) => {
  const c = document.createElement("canvas").getContext("2d");
  c.font = `${w} 100px ${f}`;
  return c.measureText(s).width;
}, [family, weight, SAMPLE]);
const plex = await width("P", 600);
for (const [name, locals] of Object.entries(FALLBACKS)) {
  for (const local of locals) {
    // A face this machine lacks falls through to monospace: skip it.
    const w = await width(`"${local}", monospace`, 400);
    if (Math.abs(w - (await width("monospace", 400))) < 0.5) continue;
    const sa = plex / w;
    const pct = n => `${(n * 100).toFixed(2)}%`;
    console.log(`${name} (${local}): size-adjust: ${pct(sa)}; ascent-override: ${pct(1.025 / sa)}; descent-override: ${pct(0.275 / sa)}; line-gap-override: 0%;`);
    break;
  }
}
await browser.close();
