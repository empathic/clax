// Screenshots of every scene in light and dark, at 1440×900 and 390×844, for
// the task named by CLAX_SHOTS (task-NN). Skipped unless it is set. Fails if
// a phone-width page scrolls sideways.
import { expect, test } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { startDaemon } from "./fixtures";
import { SCENES, seed, type Seeded } from "./scenes";

const TASK = process.env.CLAX_SHOTS ?? "";
const ONLY = process.env.CLAX_SCENES?.split(",") ?? null;
const SIZES = { desktop: { width: 1440, height: 900 }, phone: { width: 390, height: 844 } } as const;
const out = fileURLToPath(new URL(`../../.superpowers/sdd/2026-09-30-redesign/build-shots/${TASK}/`, import.meta.url));

test.skip(!/^task-\d\d$/.test(TASK), "set CLAX_SHOTS=task-NN to take the screenshots");

let d: Awaited<ReturnType<typeof startDaemon>>;
let s: Seeded;
test.beforeAll(async () => { test.setTimeout(240_000); d = await startDaemon(); s = await seed(d.base, d.token); mkdirSync(out, { recursive: true }); });
test.afterAll(async () => { await d?.stop(); });

for (const scene of SCENES) {
  for (const theme of ["light", "dark"] as const) {
    for (const [size, viewport] of Object.entries(SIZES)) {
      test(`${scene.name} ${theme} ${size}`, async ({ page }) => {
        test.skip(!!ONLY && !ONLY.includes(scene.name));
        await page.setViewportSize(viewport);
        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
        await page.goto(`${d.base}${scene.path(s)}`);
        await scene.prepare?.(page, s);
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${out}${theme}-${size}-${scene.name}.png` });
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
      });
    }
  }
}
