import { type Page } from "@playwright/test";
import { test, expect, type Daemon, commentModeIn, expectVisibleClip, last, publish, record } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const PAGE = `<main><h2>Quarterly goals</h2><p>Grow revenue and keep costs flat this quarter.</p></main>`;

async function toFrame(page: Page, msg: unknown) {
  await page.evaluate(m => (document.querySelector("iframe.frame") as HTMLIFrameElement).contentWindow!.postMessage(m, "*"), msg);
}

async function contentFrame(page: Page, id: string, n: number) {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)$`);
  await expect.poll(() => page.frame({ url }) !== null).toBe(true);
  return page.frame({ url })!;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: hover outlines, element and range picks carry anchors and clips`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Bridge ${mode}`, { "index.html": PAGE });
    await record(page);
    if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch {} });
    await page.goto(`${d.base}/a/${artifact.id}`);
    const frame = await contentFrame(page, artifact.id, 1);
    expect((await last(page, "clax:hello")).version).toBe(1);

    // Comment mode on in the shell: a pick's clip is rendered once the
    // composer its start opened has focus.
    const toggle = page.getByRole("button", { name: "Comment", exact: true });
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await commentModeIn(frame);
    await frame.locator("h2").hover();
    await expect(frame.locator("clax-overlay .o")).toBeVisible();
    expect((await last(page, "clax:hover")).selector).toBe("body > main > h2");

    await frame.locator("h2").click();
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "element", selector: "body > main > h2", quote: "Quarterly goals" });
    expect(pick.clipError).toBeUndefined();
    expect(pick.clipBytes).toBeGreaterThan(0);
    await expectVisibleClip(page, pick.pickId);
    await page.locator(".composer").getByRole("button", { name: "Cancel" }).click();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");

    const box = (await frame.locator("p").boundingBox())!;
    await page.mouse.move(box.x + 3, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 90, box.y + box.height / 2, { steps: 5 });
    await page.mouse.up();
    await expect.poll(async () => (await last(page, "clax:pick")).anchor.kind).toBe("range");
    const range = await last(page, "clax:pick");
    expect(range.anchor.quote.length).toBeGreaterThan(0);
    expect("Grow revenue and keep costs flat this quarter.").toContain(range.anchor.quote);
    expect(range.clipError).toBeUndefined();
    await expectVisibleClip(page, range.pickId);

    await toFrame(page, { type: "clax:resolve-anchors", requestId: "r1", anchors: [{ id: "t1", anchor: pick.anchor }] });
    const res = await last(page, "clax:anchors");
    expect(res.requestId).toBe("r1");
    expect(res.results[0]).toMatchObject({ id: "t1", found: true, method: "exact" });

    await page.locator(".composer").getByRole("button", { name: "Cancel" }).click();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    await frame.locator("h2").hover();
    await expect(frame.locator("clax-overlay .o")).toBeHidden();
  });
}
