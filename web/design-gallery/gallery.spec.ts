// The design gallery, section by section, in light and dark at a desktop and
// a phone width, against committed baselines (playwright.design-gallery.config.ts).
// The gallery's static build is served from CLAX_GALLERY_DIST (else
// dist-design-gallery/) by a plain file server: it needs no daemon.
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { extname, join, normalize } from "node:path";
import { expect, test } from "@playwright/test";

const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml" };
const SECTIONS = ["roles", "over", "buttons", "fields", "tags", "marks", "pins", "banners", "live", "overlay", "contrast"];

/** The build to serve: CLAX_GALLERY_DIST (made absolute by the config under Bazel), else the local build. */
const distDir = (): string => process.env.CLAX_GALLERY_DIST ?? join(__dirname, "..", "dist-design-gallery");

let server: Server;
let origin = "";

test.beforeAll(async () => {
  const root = distDir();
  expect(existsSync(join(root, "index.html")), `the gallery's build in ${root}`).toBe(true);
  server = createServer((req, res) => {
    const path = normalize(decodeURIComponent(new URL(req.url ?? "/", "http://x").pathname)).replace(/^(\.\.[/\\])+/, "");
    const file = join(root, path === "/" ? "index.html" : path);
    if (!file.startsWith(root) || !existsSync(file) || !statSync(file).isFile()) { res.writeHead(404).end(); return; }
    res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
    createReadStream(file).pipe(res);
  });
  await new Promise<void>(ok => server.listen(0, "127.0.0.1", ok));
  const at = server.address();
  origin = `http://127.0.0.1:${typeof at === "object" && at ? at.port : 0}`;
});

test.afterAll(() => new Promise<void>(ok => server.close(() => ok())));

for (const theme of ["light", "dark"] as const) {
  for (const [width, label] of [[1280, "desktop"], [390, "phone"]] as const) {
    test(`the gallery, ${theme}, ${label}`, async ({ browser }) => {
      // The overlay follows the system's scheme: the page's colour scheme matches the theme.
      const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: theme, deviceScaleFactor: 1, reducedMotion: "reduce" });
      const page = await context.newPage();
      const errors: string[] = [];
      page.on("pageerror", e => errors.push(String(e)));
      await page.goto(`${origin}/index.html?theme=${theme}`);
      await expect(page.locator("main.dg h1")).toHaveText("Clax design gallery");
      // The overlay's panels mounted in their shadow root, styled by its tokens, not the host page's.
      const panel = page.locator(".overlay-host").locator("section.panel");
      await expect(panel).toBeVisible();
      const hostMuted = await page.locator(".store-text").evaluate(el => getComputedStyle(el).color);
      const hostFg = "rgb(0, 160, 0)";
      const overlayFg = await panel.locator("h2").evaluate(el => getComputedStyle(el).color);
      expect(hostMuted).toBe("rgb(255, 0, 255)");
      expect(overlayFg).not.toBe(hostFg);
      expect(overlayFg).toBe(theme === "dark" ? "rgb(236, 235, 231)" : "rgb(28, 27, 25)");
      for (const id of SECTIONS) {
        const heading = page.locator(`h2#${id}`);
        const body = heading.locator("xpath=following-sibling::*[2]");
        await expect(body, id).toHaveScreenshot(`${id}-${theme}-${label}.png`);
      }
      expect(errors).toEqual([]);
      await context.close();
    });
  }
}
