// The live layers (spec 2026-10-11-clax-dev-hosted-design §16.3) between two
// people in two browsers on one hosted daemon (a local daemon's browsers are
// all its owner): Mia's pointer, text selection,
// comment-mode pick, typing and drafts show in Alex's window as she makes
// them, named only by the shell (never inside the page), and the open
// threads' quotes are highlighted in the page. With CLAX_LIVE_SHOTS set to a
// directory, Alex's view is also photographed at 1280 and 390 px wide, light
// and dark.
import { mkdirSync } from "node:fs";
import { type Frame, type Page } from "@playwright/test";
import { test, expect, commentModeIn, publish } from "./fixtures";
import { hostedDaemon, memberOf, openAt, ownerOf, rightsOn } from "./hosted-helpers";

const SHOTS = process.env.CLAX_LIVE_SHOTS ?? "";

const PAGE = `<main style="font: 18px/1.6 system-ui; max-width: 640px; margin: 40px auto; padding: 0 16px">
<h2>Quarterly goals</h2>
<p id="p">Revenue grew by twelve percent this quarter, ahead of the plan.</p>
<section id="box" style="height: 160px; border: 1px solid #ccc; border-radius: 8px; padding: 12px">Pipeline chart</section>
<div style="height: 1600px"></div>
</main>`;

/** Posts a thread on `quote` in the paragraph, from `page`'s browser. */
async function quoteThread(page: Page, aid: string, body: string) {
  return page.evaluate(async ([id, text]) => {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "range", selector: "#p", quote: "twelve percent", prefix: "Revenue grew by ", suffix: " this quarter", html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    form.set("body", text);
    form.set("version", "1");
    const res = await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: form });
    if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
    return (await res.json()).thread as { id: string };
  }, [aid, body] as const);
}

/** What the live layers drew in `frame`'s page: counts by kind, and whether the quotes are highlighted. */
async function drawn(frame: Frame) {
  return frame.evaluate(() => {
    const root = document.querySelector("clax-live")?.shadowRoot;
    const n = (sel: string) => root?.querySelectorAll(sel).length ?? 0;
    const hl = (globalThis as { CSS?: { highlights?: Map<string, unknown> } }).CSS?.highlights;
    return { arrows: n(".c"), ranges: n(".s"), elements: n(".e"), areas: n(".a"), quotes: hl?.has("clax-quote") ?? false, text: root?.textContent ?? "" };
  });
}

/** Where the live layers drew the one peer's arrow in `frame`'s page (page
 * coordinates, its tip: the arrow's box is placed 2 px above it), as set
 * (it moves there with a transition). */
async function arrowAt(frame: Frame) {
  return frame.evaluate(() => {
    const a = document.querySelector("clax-live")!.shadowRoot!.querySelector<HTMLElement>(".c")!;
    const m = /translate\(([-\d.]+)px, ([-\d.]+)px\)/.exec(a.style.transform)!;
    return { x: Number(m[1]), y: Number(m[2]) + 2 };
  });
}

/** Where the shell put the one name tag over the frame (stage pixels), as set. */
async function tagAt(page: Page) {
  return page.locator(".peer-tag").evaluate(el => {
    const m = /translate\(([-\d.]+)px, ([-\d.]+)px\)/.exec((el as HTMLElement).style.transform)!;
    return { x: Number(m[1]) - 12, y: Number(m[2]) - 12 };
  });
}

/** The boxes (page coordinates) of what the live layers drew of kind `cls` in `frame`'s page. */
async function boxes(frame: Frame, cls: string) {
  return frame.evaluate(c => Array.from(document.querySelector("clax-live")!.shadowRoot!.querySelectorAll<HTMLElement>(`.${c}`), el => ({
    x: parseFloat(el.style.left), y: parseFloat(el.style.top), w: parseFloat(el.style.width), h: parseFloat(el.style.height),
  })), cls);
}

const near = (a: number, b: number, by: number) => expect(Math.abs(a - b), `${a} within ${by} of ${b}`).toBeLessThanOrEqual(by);

/** The frame element's box in the shell, and `selector`'s in the page. */
async function pointAt(page: Page, frame: Frame, selector: string) {
  const f = (await page.locator("iframe.frame").boundingBox())!;
  const r = await frame.evaluate(s => { const b = document.querySelector(s)!.getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; }, selector);
  return { x: f.x + r.x + Math.min(40, r.w / 2), y: f.y + r.y + r.h / 2 };
}

async function threads(page: Page) {
  const phone = page.locator(".phone-tabs button", { hasText: "Threads" });
  const toggle = (await phone.isVisible()) ? phone : page.locator("button.threads");
  if ((await toggle.getAttribute("aria-pressed")) !== "true") await toggle.click();
  await expect(page.locator("aside.sidebar")).toBeVisible();
}

test("one person's pointer, selection, pick, typing and drafts show in the other's window", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
  const { artifact } = await publish(d.base, d.token, "Live", { "index.html": PAGE });
  const alex = await ownerOf(browser, origin, "Alex");
  const mia = await memberOf(browser, alex, origin, "Mia", rightsOn(artifact.id));
  const [alexCtx, miaCtx] = [alex.context(), mia.context()];
  await alex.setViewportSize({ width: 1280, height: 800 });
  await mia.setViewportSize({ width: 1280, height: 800 });
  const t = await quoteThread(alex, artifact.id, "Is this year on year?");
  const af = await openAt(alex, origin, artifact.id, 1);
  const mf = await openAt(mia, origin, artifact.id, 1);

  // The open thread's quote is highlighted in the page.
  await expect.poll(async () => (await drawn(af)).quotes).toBe(true);

  // Mia's pointer: an unnamed arrow in Alex's page, her name over it in the shell.
  await expect.poll(async () => {
    const p = await pointAt(mia, mf, "#p");
    await mia.mouse.move(p.x, p.y);
    await mia.mouse.move(p.x + 3, p.y);
    return (await drawn(af)).arrows;
  }).toBe(1);
  const tag = alex.locator(".peer-tag");
  await expect(tag).toHaveText("Mia");
  const inPage = await drawn(af);
  expect(inPage.text).not.toContain("Mia");
  expect(await af.evaluate(() => document.documentElement.outerHTML)).not.toContain("Mia");
  // The arrow is where Mia's pointer is on the paragraph's text, placed by
  // its text offset: at the caret nearest her pointer, at most CARET_NEAR
  // (24 px) across from it and within the line, measured from the
  // paragraph's corner in each page (the paragraph is 640 px wide in both).
  const mfBox = (await mia.locator("iframe.frame").boundingBox())!;
  const p0 = await pointAt(mia, mf, "#p");
  const corner = (f: Frame) => f.evaluate(() => { const b = document.querySelector("#p")!.getBoundingClientRect(); return { x: b.left + scrollX, y: b.top + scrollY }; });
  const [mc, ac] = [await corner(mf), await corner(af)];
  const miaAt = { x: p0.x + 3 - mfBox.x - mc.x, y: p0.y - mfBox.y - mc.y };
  await expect.poll(async () => { const a = await arrowAt(af); return Math.abs(a.x - ac.x - miaAt.x) <= 24 && Math.abs(a.y - ac.y - miaAt.y) <= 16; }).toBe(true);
  // The name tag is over the arrow: the frame fills Alex's stage, and his page is not scrolled.
  await expect.poll(async () => { const [a, g] = [await arrowAt(af), await tagAt(alex)]; return Math.abs(a.x - g.x) <= 1 && Math.abs(a.y - g.y) <= 1; }).toBe(true);
  // Alex scrolls his page: the arrow stays on the paragraph, and the tag moves up with it.
  const before = await arrowAt(af);
  await af.evaluate(() => scrollTo(0, 120));
  await expect.poll(async () => (await tagAt(alex)).y).toBe(Math.round(before.y - 120));
  expect(await arrowAt(af)).toEqual(before);
  await af.evaluate(() => scrollTo(0, 0));
  await expect.poll(async () => (await tagAt(alex)).y).toBe(Math.round(before.y));
  // Alex's window narrows: the page reflows, and the arrow follows the
  // paragraph's text to where it is laid out now.
  const pLeft = () => af.evaluate(() => document.querySelector("#p")!.getBoundingClientRect().left);
  const left0 = await pLeft();
  await alex.setViewportSize({ width: 1000, height: 800 });
  await expect.poll(pLeft).not.toBe(left0);
  const shift = (await pLeft()) - left0;
  await expect.poll(async () => Math.abs((await arrowAt(af)).x - (before.x + shift)) <= 1).toBe(true);
  await alex.setViewportSize({ width: 1280, height: 800 });
  await expect.poll(async () => Math.abs((await arrowAt(af)).x - before.x) <= 1).toBe(true);

  // Mia's text selection: tinted in Alex's page by element path and offset.
  await mf.evaluate(() => {
    const n = document.querySelector("#p")!.firstChild!;
    getSelection()!.setBaseAndExtent(n, 16, n, 30);
  });
  await expect.poll(async () => (await drawn(af)).ranges).toBeGreaterThan(0);
  // Tinted exactly over "twelve percent" in Alex's page.
  const want = await af.evaluate(() => {
    const n = document.querySelector("#p")!.firstChild!;
    const r = document.createRange();
    r.setStart(n, 16);
    r.setEnd(n, 30);
    return Array.from(r.getClientRects(), b => ({ x: b.left + scrollX, y: b.top + scrollY, w: b.width, h: b.height })).filter(b => b.w > 0);
  });
  const got = await boxes(af, "s");
  expect(got).toHaveLength(want.length);
  got.forEach((b, i) => { near(b.x, want[i]!.x, 0.5); near(b.y, want[i]!.y, 0.5); near(b.w, want[i]!.w, 0.5); near(b.h, want[i]!.h, 0.5); });
  await mf.evaluate(() => getSelection()!.removeAllRanges());
  await expect.poll(async () => (await drawn(af)).ranges).toBe(0);

  // Mia picks an element in comment mode: Alex sees it outlined while she writes.
  await mia.getByRole("button", { name: /^Comment/ }).first().click();
  await commentModeIn(mf);
  const box = await pointAt(mia, mf, "#box");
  await mia.mouse.move(box.x, box.y);
  await mia.mouse.click(box.x, box.y);
  const composer = mia.locator(".stage .composer");
  await expect(composer.locator("textarea")).toBeFocused();
  await expect.poll(async () => (await drawn(af)).elements).toBe(1);
  // Outlined on the section's own box in Alex's page.
  const sec = await af.evaluate(() => { const b = document.querySelector("#box")!.getBoundingClientRect(); return { x: b.left + scrollX, y: b.top + scrollY, w: b.width, h: b.height }; });
  const [outline] = await boxes(af, "e");
  near(outline!.x, sec.x, 0.5); near(outline!.y, sec.y, 0.5); near(outline!.w, sec.w, 0.5); near(outline!.h, sec.h, 0.5);
  await mia.keyboard.type("Label the axes");
  await threads(alex);
  const newWriters = alex.locator(".new-writers .live-writers");
  await expect(newWriters).toContainText("Mia");
  await expect(newWriters).toContainText("Label the axes");
  if (SHOTS) {
    mkdirSync(SHOTS, { recursive: true });
    await alex.screenshot({ path: `${SHOTS}/live-1280-light-pick.png` });
  }
  await composer.getByRole("button", { name: "Cancel" }).click();
  await expect(newWriters).toHaveCount(0);
  await expect.poll(async () => (await drawn(af)).elements).toBe(0);

  // Mia types a reply in the thread: "Mia is typing…" and her draft on Alex's card.
  await threads(mia);
  const reply = mia.locator(`.thread-card[data-thread="${t.id}"] input[aria-label="Reply"]`);
  await reply.click();
  await reply.pressSequentially("Yes, year on year");
  const writers = alex.locator(`.thread-card[data-thread="${t.id}"] .live-writers`);
  await expect(writers).toContainText("Mia");
  await expect(writers).toContainText("is typing…");
  await expect(writers).toContainText("Yes, year on year");

  if (SHOTS) {
    mkdirSync(SHOTS, { recursive: true });
    const p = await pointAt(mia, mf, "#p");
    await mia.mouse.move(p.x + 60, p.y);
    await mf.evaluate(() => { const n = document.querySelector("#p")!.firstChild!; getSelection()!.setBaseAndExtent(n, 16, n, 30); });
    await expect.poll(async () => (await drawn(af)).ranges).toBeGreaterThan(0);
    // The tag has stopped moving (it follows the pointer with a transition).
    const settled = async () => {
      let last = "";
      await expect.poll(async () => { const b = JSON.stringify(await tag.boundingBox()); const same = b === last; last = b; return same; }).toBe(true);
    };
    for (const scheme of ["light", "dark"] as const) {
      await alex.emulateMedia({ colorScheme: scheme });
      for (const width of [1280, 390]) {
        await alex.setViewportSize({ width, height: 800 });
        await threads(alex);
        await expect(writers).toBeVisible();
        if (width === 1280) await settled();
        await alex.screenshot({ path: `${SHOTS}/live-${width}-${scheme}-threads.png` });
        if (width === 390) {
          await alex.locator(".phone-tabs button", { hasText: "Page" }).click();
          await expect.poll(async () => (await drawn(af)).arrows).toBe(1);
          await settled();
          await alex.screenshot({ path: `${SHOTS}/live-${width}-${scheme}-page.png` });
        }
      }
    }
    await alex.setViewportSize({ width: 1280, height: 800 });
    await threads(alex);
  }

  // Posting the reply ends the draft for Alex.
  await reply.press("Enter");
  await expect(writers).toHaveCount(0);
  await expect(alex.locator(`.thread-card[data-thread="${t.id}"]`)).toContainText("Yes, year on year");

  // Mia leaves: her pointer and name go.
  await miaCtx.close();
  await expect(tag).toHaveCount(0);
  await expect.poll(async () => (await drawn(af)).arrows).toBe(0);
  await alexCtx.close();
  } finally {
    await d.stop();
  }
});
