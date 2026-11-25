// The clax-timing rules (timing.mjs) over code each must pass and code each
// must refuse, with oxlint's own RuleTester.
import { describe, it } from "vitest";
import { RuleTester } from "oxlint/plugins-dev";
import plugin from "./timing.mjs";

RuleTester.describe = describe;
RuleTester.it = it;
const tester = new RuleTester();
const ts = (code: string) => ({ code, filename: "x.test.ts" });
const refused = (code: string, errors = 1) => ({ ...ts(code), errors });

tester.run("timer-delay", plugin.rules["timer-delay"], {
  valid: [
    ts("await new Promise(r => setTimeout(r, 0));"),
    ts("await new Promise(r => setTimeout(r));"),
    ts("const stuck = setTimeout(() => fail(), HANG_MS);"),
    ts("window.setTimeout(f, WAIT_MS * 2);"),
    ts("vi.useFakeTimers(); setTimeout(f, 500);"),
    ts("test.setTimeout(5000);"),
    ts('import { setTimeout as sleep } from "node:timers/promises"; await sleep(0);'),
  ],
  invalid: [
    refused("await new Promise(r => setTimeout(r, 50));"),
    refused("setInterval(poll, 100);"),
    refused("globalThis.setTimeout(f, ms);"),
    refused('import { setTimeout as sleep } from "node:timers/promises"; await sleep(100);'),
  ],
});

tester.run("literal-timeout", plugin.rules["literal-timeout"], {
  valid: [
    ts("await expect(el).toBeVisible({ timeout: 0 });"),
    ts("await expect(el).toBeVisible({ timeout: WAIT_MS });"),
    ts("await expect.poll(f, { timeout: HANG_MS * 2 }).toBe(1);"),
    ts("await el.click({ timeout: opts.timeout });"),
    ts('it("runs", async () => {});'),
    ts("beforeAll(async () => {}, HANG_MS);"),
    ts("setTimeout(f, 50);"),
  ],
  invalid: [
    refused("await expect(el).toBeVisible({ timeout: 10_000 });"),
    refused("const LONG = 60 * 1000; await expect.poll(f, { timeout: LONG }).toBe(1);"),
    refused("await vi.waitFor(f, { timeout: 3000 });"),
    refused("test.setTimeout(90_000);"),
    refused("page.setDefaultTimeout(5000);"),
    refused('it("runs", async () => {}, 20_000);'),
    refused("beforeAll(async () => {}, 320_000);"),
  ],
});

tester.run("clock-arithmetic", plugin.rules["clock-arithmetic"], {
  valid: [
    ts("const t0 = Date.now(); await f(); console.log(`took ${Date.now() - t0} ms`);"),
    ts("const hourAgo = Date.now() - 3_600_000;"),
    ts("const end = Date.now() + HANG_MS; while (Date.now() < end) await poll();"),
    ts("if (Date.now() > start + WAIT_MS) throw new Error('stuck');"),
  ],
  invalid: [
    refused("const t0 = performance.now(); await f(); expect(performance.now() - t0).toBeLessThan(500);"),
    refused("const t0 = Date.now(); await f(); const ms = Date.now() - t0; expect(ms).toBeLessThan(500);"),
    refused("if (Date.now() - t0 > 50) throw new Error('slow');"),
    refused("const end = Date.now() + 5000; while (Date.now() < end) await poll();"),
  ],
});

tester.run("disabled-test", plugin.rules["disabled-test"], {
  valid: [
    ts('test.skip(!process.env.CLAX_E2E_SHOTS, "set CLAX_E2E_SHOTS to take them");'),
    ts('test.skip(({ browserName }) => browserName !== "chromium", "Chromium only");'),
    ts('it.skipIf(!node)("runs node", () => {});'),
  ],
  invalid: [
    refused('test.fixme("flaky", async () => {});'),
    refused("test.fixme();"),
    refused('test.skip("later", async () => {});'),
    refused('describe.skip("later", () => {});'),
    refused("test.skip(true, 'off');"),
    refused('xit("later", () => {});'),
  ],
});

tester.run("counted-turns", plugin.rules["counted-turns"], {
  valid: [
    ts("await tick();"),
    ts("for (let i = 0; i < 5; i++) { await tick(); clock.advance(1000); }"),
    ts("for (let i = 0; i < 6; i++) await page.keyboard.press('ArrowLeft');"),
    ts("await settled();"),
  ],
  invalid: [
    refused("for (let i = 0; i < 5; i++) await tick();"),
    refused("for (let i = 0; i < 3; i++) await Promise.resolve();"),
    refused("for (let i = 0; i < 10; i++) { await new Promise(r => setTimeout(r, 0)); }"),
    refused("for (let i = 0; i < 4; i++) await vi.advanceTimersByTimeAsync(0);"),
    refused("await settle(3);"),
  ],
});
