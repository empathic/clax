import { test, expect, type Frame, type Page } from "@playwright/test";
import { reach, api, openArtifact, publish, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const IMG = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><rect width="300" height="200" fill="#16a34a"/><rect x="100" y="60" width="80" height="80" fill="#dc2626"/></svg>`)}`;
const STYLE = `<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px}#pic{display:block}</style>`;
const INDEX = `<!doctype html><html><head><title>Again</title>${STYLE}</head><body><main><h1 id="title">Quarterly goals</h1><p id="para">Grow revenue and keep costs flat.</p><img id="pic" src="${IMG}" width="300" height="200"><p><a id="to-about" href="about.html">About</a></p></main></body></html>`;
const ABOUT = `<!doctype html><html><head><title>About</title>${STYLE}</head><body><main><h2 id="team">Our team</h2><p id="we">We build things.</p></main></body></html>`;

/** Whether the comment-mode hover outline is shown in the page. */
const outlined = (frame: Frame) => frame.evaluate(() => document.querySelector("clax-overlay")?.shadowRoot?.querySelector<HTMLElement>(".o")?.style.display === "block");

async function post(page: Page, text: string) {
  const composer = page.locator(".composer");
  await composer.locator("textarea").fill(text);
  await composer.getByRole("button", { name: "Post comment" }).click();
  await expect(page.locator(".thread-card").filter({ hasText: text })).toHaveCount(1);
  await expect(composer).toHaveCount(0);
}

/** Comment mode is back on after a post: the button shows pressed, the page
 * shows the crosshair, and hovering `sel` outlines it again. */
async function backOn(page: Page, frame: Frame, sel: string) {
  const toggle = page.getByRole("button", { name: "Comment", exact: true });
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  await reach(page, frame.locator(sel));
  await frame.locator(sel).hover();
  await expect.poll(() => outlined(frame)).toBe(true);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: comment mode comes back after each post, so the viewer comments on an element, then an area, then on a second page, pressing Comment once per page`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Again ${mode}`, { "index.html": INDEX, "about.html": ABOUT });
    const id = artifact.id;
    const frame = await openArtifact(page, d.base, id, 1, mode);
    if (mode === "sandbox") await expect(page.locator("iframe.frame")).toHaveAttribute("sandbox", /allow-scripts/);
    else await expect(page.locator("iframe.frame")).not.toHaveAttribute("sandbox", /.*/);
    const toggle = page.getByRole("button", { name: "Comment", exact: true });
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");

    // First: an element. Comment mode is off while the composer is open.
    await frame.locator("#title").hover();
    await expect.poll(() => outlined(frame)).toBe(true);
    await frame.locator("#title").click();
    await expect(page.locator(".composer .composer-quote")).toContainText("Quarterly goals");
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    await post(page, "Which quarter?");
    await backOn(page, frame, "#para");
    await page.screenshot({ path: test.info().outputPath(`${mode}-after-first-post.png`) });

    // Second, with no press of Comment: an area drawn over the image.
    const pic = await frame.evaluate(() => { const b = document.querySelector("#pic")!.getBoundingClientRect(); return { x: b.x, y: b.y }; });
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    await page.mouse.move(fb.x + pic.x + 90, fb.y + pic.y + 50);
    await page.mouse.down();
    await page.mouse.move(fb.x + pic.x + 150, fb.y + pic.y + 100, { steps: 4 });
    await page.mouse.move(fb.x + pic.x + 210, fb.y + pic.y + 150, { steps: 4 });
    await page.mouse.up();
    await expect(page.locator(".composer .composer-quote")).toHaveText("Area in #pic (40% × 50%)");
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    await post(page, "What is this red square?");
    await backOn(page, frame, "#title");
    await page.screenshot({ path: test.info().outputPath(`${mode}-after-area-post.png`) });

    const { threads } = await api(d.base, d.token, `/api/artifacts/${id}/threads`) as { threads: { anchor: { kind: string; selector: string } }[] };
    expect(threads.map(t => [t.anchor.kind, t.anchor.selector]).sort()).toEqual([["area", "#pic"], ["element", "#title"]]);

    // Cancel brings it back too; Escape with no composer open ends it.
    await frame.locator("#para").click();
    await expect(page.locator(".composer")).toHaveCount(1);
    await page.locator(".composer").getByRole("button", { name: "Cancel" }).click();
    await backOn(page, frame, "#title");
    await page.locator("header h1").hover();
    await page.keyboard.press("Escape");
    await expect(toggle).toHaveAttribute("aria-pressed", "false");

    // A second page of the artifact behaves the same.
    await frame.locator("#to-about").click();
    const aboutUrl = new RegExp(`(${id}\\.localhost:\\d+/v/1/about\\.html|/c/${id}/v/1/about\\.html)$`);
    await expect.poll(() => page.frame({ url: aboutUrl }) !== null).toBe(true);
    const about = page.frame({ url: aboutUrl })!;
    await expect(about.locator("#team")).toHaveText("Our team");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await about.locator("#team").hover();
    await about.locator("#team").click();
    await expect(page.locator(".composer .file-label")).toHaveText("on about.html");
    await post(page, "Name the team.");
    await backOn(page, about, "#we");
    await about.locator("#we").click();
    await expect(page.locator(".composer .composer-quote")).toContainText("We build things.");
    await post(page, "What things?");
    await backOn(page, about, "#team");
  });
}
