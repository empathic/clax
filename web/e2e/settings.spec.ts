// Settings (`/settings`, docs/usage.md "Settings") on a local daemon, as
// its owner: the gear at the header's end, the overview and its search, a
// result opening its screen at the heading it names, moving between
// screens in place, the search dialog on the Settings key from any page of
// the gallery, and the overview and a screen at phone width. Each screen's
// own behaviour is its page's spec's (people.spec.ts, claxbot.spec.ts,
// claxbot-daemon.spec.ts, hosted.spec.ts).
import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";

/** The elements past the page's right edge that no scrolling box holds. */
const sideways = (page: Page) => page.evaluate(() => {
  const w = document.documentElement.clientWidth;
  if (document.documentElement.scrollWidth <= w) return [];
  const scrolls = (el: Element | null): boolean => !!el && (getComputedStyle(el).overflowX !== "visible" || scrolls(el.parentElement));
  return [...document.body.querySelectorAll("*")].filter(el => el.getBoundingClientRect().right > w + 0.5 && !scrolls(el.parentElement)).map(el => `${el.tagName.toLowerCase()}.${[...el.classList].join(".")}`).slice(0, 8);
});

test("the gear opens Settings, whose search opens a setting at its heading", async ({ page, daemon: d }, info) => {
  await page.goto(d.base);
  const gear = page.locator(".gbar a.settings-link");
  await expect(gear).toHaveAccessibleName("Settings");
  // The gear is the header's last control.
  expect(await page.locator(".gbar > :last-child").getAttribute("class")).toContain("settings-link");
  await gear.click();
  await page.waitForURL(`${d.base}/settings`);
  await expect(page.getByRole("heading", { name: "Settings", level: 2 })).toBeVisible();
  await expect(gear).toHaveAttribute("aria-current", "page");
  // Every screen this owner sees, by group (ClaxBot's too, where it is on).
  await expect(page.locator(".stg-over h3").first()).toHaveText("You");
  await expect(page.locator(".stg-over h3").last()).toHaveText("System");
  for (const name of ["Agents & machines", "Confirmations", "People", "Configuration", "Remotes", "Audit log"]) {
    await expect(page.locator(".stg-cards b").filter({ hasText: new RegExp(`^${name}$`) })).toBeVisible();
  }
  await page.screenshot({ path: info.outputPath("overview.png") });

  // A word no title carries finds the setting, and says where it is.
  const field = page.getByRole("combobox", { name: "Search settings" });
  await field.fill("webauthn");
  await expect(page.getByRole("option")).toHaveText(["Require a passkey to confirmYou › Confirmations"]);
  await field.fill("passkey");
  await expect(page.getByRole("option").first()).toContainText("Passkeys");
  await page.screenshot({ path: info.outputPath("search.png") });
  await field.press("Enter");
  await page.waitForURL(`${d.base}/settings/confirmations#pk-h`);
  await expect(page.locator("#pk-h")).toBeFocused();
  const nav = page.getByRole("navigation", { name: "Settings" });
  await expect(nav.locator("[aria-current=page]")).toHaveText("Confirmations");

  // The navigation moves between screens without loading the page again.
  await page.evaluate(() => { (window as unknown as { kept: boolean }).kept = true; });
  await nav.getByRole("link", { name: "People", exact: true }).click();
  await page.waitForURL(`${d.base}/settings/people`);
  await expect(page.getByRole("heading", { name: "People", level: 2 })).toBeVisible();
  await expect(page.getByRole("button", { name: "Invite a person" })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("heading", { name: "Confirmations", level: 2 })).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { kept?: boolean }).kept)).toBe(true);
  await page.screenshot({ path: info.outputPath("screen.png") });
});

test("the Settings key opens the search over any gallery page and goes to a screen by name", async ({ page, daemon: d }, info) => {
  await page.goto(d.base);
  await expect(page.locator(".gbar a.settings-link")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  const dialog = page.getByRole("dialog", { name: "Search settings" });
  await expect(dialog.getByRole("combobox")).toBeFocused();
  // Before anything is typed it lists the screens.
  await expect(dialog.getByRole("option").first()).toContainText("Agents & machines");
  await page.screenshot({ path: info.outputPath("palette.png") });
  await page.keyboard.type("audit");
  await expect(dialog.getByRole("option").first()).toContainText("Audit log");
  await page.keyboard.press("Enter");
  await page.waitForURL(`${d.base}/settings/audit`);
  await expect(page.getByRole("heading", { name: "Audit log", level: 2 })).toBeVisible();

  // In Settings the same key opens it; the arrow keys choose, and the choice opens in place.
  await page.keyboard.press("ControlOrMeta+k");
  await expect(dialog.getByRole("combobox")).toBeFocused();
  await page.keyboard.type("remotes");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await page.waitForURL(`${d.base}/settings/remotes`);
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Remotes", level: 2 })).toBeVisible();

  // Escape closes it and gives the focus back.
  const search = page.getByRole("button", { name: /^Search settings/ }).first();
  await search.click();
  await expect(dialog.getByRole("combobox")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(search).toBeFocused();
});

test("Settings fits a phone: the overview, then a screen under a link back to it", async ({ page, daemon: d }, info) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${d.base}/settings`);
  await expect(page.locator(".stg-cards b").first()).toBeVisible();
  expect(await sideways(page)).toEqual([]);
  await page.screenshot({ path: info.outputPath("phone-overview.png") });
  await page.getByRole("link", { name: /^People/ }).click();
  await page.waitForURL(`${d.base}/settings/people`);
  await expect(page.getByRole("navigation", { name: "Settings" })).toBeHidden();
  await expect(page.getByRole("button", { name: "Invite a person" })).toBeVisible();
  expect(await sideways(page)).toEqual([]);
  await page.screenshot({ path: info.outputPath("phone-screen.png") });
  await page.getByRole("button", { name: "Search settings" }).click();
  await expect(page.getByRole("dialog", { name: "Search settings" })).toBeVisible();
  expect(await sideways(page)).toEqual([]);
  await page.keyboard.press("Escape");
  await page.getByRole("link", { name: "‹ All settings" }).click();
  await page.waitForURL(`${d.base}/settings`);
  await expect(page.getByRole("heading", { name: "Settings", level: 2 })).toBeVisible();
});
