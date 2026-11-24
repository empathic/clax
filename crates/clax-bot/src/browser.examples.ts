// Scripts as Playwright's documentation writes them, using every member
// of the browser declaration `api::dts` writes for a run holding every
// browser capability (src/api.browser-all.d.ts.golden). A test compiles
// them against that declaration with TypeScript
// (web/scripts/claxbot-browser-dts.test.ts): a member renamed, or a
// signature changed, in crates/clax-bot/src/browser/layer.rs fails it.
// Each is the body of a script, so each is an async function here.

export async function checkTheBookingForm() {
  const page = await clax.browser.open({ artifact: "draft" }, { viewport: "phone" });
  await page.getByLabel("Email").fill("ada@example.com");
  await page.getByRole("button", { name: "Book" }).click();
  await expect(page.getByText("Booked")).toBeVisible();
  const shot = await page.screenshot({ fullPage: true });
  const errors = await page.pageErrors();
  await page.close();
  return { shot, errors };
}

export async function everyPageMember() {
  const page = await clax.browser.open({ url: "https://example.com/" });
  const response = await page.goto("/about", { waitUntil: "domcontentloaded", timeout: 10000 });
  if (response) {
    const seen: [string, number, boolean] = [response.url(), response.status(), response.ok()];
    console.log(seen);
  }
  await page.reload({ waitUntil: "load" });
  await page.goBack({ timeout: 5000 });
  await page.goForward();
  const url: string = page.url();
  const title: string = await page.title();
  const html: string = await page.content();
  await page.setViewportSize({ width: 800, height: 600 });
  page.setDefaultTimeout(5000);
  await page.waitForLoadState("networkidle", { timeout: 1000 });
  await page.waitForURL(/about/, { timeout: 1000 });
  const sum: number = await page.evaluate((n: number) => n + 1, 41);
  const href: string = await page.evaluate("location.href");
  const messages = await page.consoleMessages();
  for (const m of messages) console.log(m.type(), m.text(), m.location().url);
  const failed = await page.failedRequests();
  console.log(failed.failed.length, failed.blocked.map((b) => b.why));
  const results = await page.runTests(["tests/form.test.js"]);
  console.log(results.passed, results.failed, results.skipped);
  await page.keyboard.down("Shift");
  await page.keyboard.up("Shift");
  await page.keyboard.press("Enter");
  await page.keyboard.type("hello");
  await page.keyboard.insertText("é");
  await page.mouse.move(10, 20, { steps: 5 });
  await page.mouse.down({ button: "left" });
  await page.mouse.up();
  await page.mouse.click(10, 20, { button: "right", clickCount: 2 });
  await page.mouse.dblclick(10, 20);
  await page.mouse.wheel(0, 300);
  await expect(page).toHaveURL(/about/);
  await expect(page).not.toHaveTitle("Error", { timeout: 2000 });
  const closed = await page.close();
  return { url, title, html, sum, href, closed: closed.ms };
}

export async function everyLocatorMember() {
  const page = await clax.browser.open({ html: "<form></form>", files: { "a.css": "p {}" } });
  const form = page.locator("form");
  const row = form
    .getByRole("row", { name: /total/i, exact: false, checked: true, level: 2, includeHidden: false })
    .getByText("Sum", { exact: true })
    .getByLabel(/amount/)
    .getByPlaceholder("0.00")
    .getByTestId("total")
    .locator("span")
    .filter({ hasText: "€", hasNotText: /draft/ });
  const rows = page.getByTestId(/row-/);
  await rows.first().click({ button: "left", clickCount: 1, position: { x: 1, y: 2 }, timeout: 1000 });
  await rows.last().dblclick({ position: { x: 1, y: 1 } });
  await rows.nth(2).hover({ timeout: 500 });
  await row.fill("12", { timeout: 1000 });
  await row.press("Tab");
  await row.pressSequentially("34");
  await row.focus();
  await row.check();
  await row.uncheck();
  const picked: string[] = await row.selectOption(["a", { label: "B" }, { index: 2 }]);
  const text: string | null = await row.textContent();
  const inner: string = await row.innerText();
  const value: string = await row.inputValue();
  const attr: string | null = await row.getAttribute("href");
  const all: string[] = await rows.allTextContents();
  const flags: boolean[] = [await row.isVisible(), await row.isHidden(), await row.isEnabled(), await row.isChecked()];
  const n: number = await rows.count();
  await row.waitFor({ state: "visible", timeout: 1000 });
  await row.scrollIntoViewIfNeeded();
  const shot = await row.screenshot();
  await expect(row).toBeVisible();
  await expect(row).toBeHidden({ timeout: 100 });
  await expect(row).toBeEnabled();
  await expect(row).toBeDisabled();
  await expect(row).toBeChecked({ checked: false });
  await expect(row).toBeFocused();
  await expect(rows).toHaveText(["one", /two/]);
  await expect(row).toContainText("1");
  await expect(row).toHaveValue(/\d+/);
  await expect(row).toHaveAttribute("aria-label", "Total");
  await expect(rows).toHaveCount(3);
  await expect(row).not.toBeVisible();
  expect(n).toBe(3);
  expect(all).toEqual(["one", "two"]);
  expect(text).toBeTruthy();
  expect(inner).toContain("1");
  expect(n).toBeGreaterThan(1);
  expect(picked).not.toContain("z");
  await page.close();
  return { value, attr, flags, shot };
}

export async function aLaterScriptReachesItsSession(id: string) {
  const page = clax.browser.session(id);
  await page.getByRole("button", { name: "Next" }).click();
  return page;
}

export async function theSavedTestsInOneCall() {
  const r = await clax.browser.test({ artifact: "draft" }, { viewport: "desktop", tests: ["tests/a.test.js"], timeout: 30000 });
  return r.passed;
}
