// What the report queue end to end tests share (e2e/queues.spec.ts,
// e2e/feedback-loop.spec.ts): where their screenshots go, the owner's agent
// on the triage routes, a site user's browser with the overlay, and the
// owner's queue with its service grant handed to the deployer's site.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { Browser, Page, TestInfo } from "@playwright/test";
import type { DeployerSite } from "./deployer-site/server";
import { expect } from "./fixtures";

/** Where a screenshot named `name` goes. */
export function shotPath(info: TestInfo, name: string): string {
  const dir = process.env.CLAX_QUEUES_SHOTS_DIR;
  if (!dir) return info.outputPath(`${name}.png`);
  mkdirSync(dir, { recursive: true });
  return join(dir, `${name}.png`);
}

/** The owner's agent: an agent token (`Bearer cxa_…`) on the triage routes,
 * from no browser (no `Origin`, no cookie), in a session it registers
 * (`POST /api/sessions`) and names on every call (`X-Clax-Session` and the
 * session's secret): a claim's lease is its session's. `ok` answers a
 * call's JSON and fails the test on any status from 300 up; `send`
 * answers a call's status and JSON whatever the status; `bytes` answers a
 * GET's status, type and body. */
export async function agentOf(origin: string, token: string) {
  const reg = await fetch(`${origin}/api/sessions`, {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify({ harness: "claude", harness_session_id: `e2e-queues-${Math.random().toString(36).slice(2)}`, cwd: "/tmp" }),
  });
  const regText = await reg.text();
  expect(reg.status, `POST /api/sessions: ${regText}`).toBe(201);
  const { session, session_secret: secret } = JSON.parse(regText) as { session: { id: string }; session_secret?: string };
  const auth: Record<string, string> = { authorization: `Bearer ${token}`, "x-clax-session": session.id };
  if (secret) auth["x-clax-session-secret"] = secret;
  return {
    sessionId: session.id,
    async ok<T>(method: string, path: string, body?: unknown): Promise<T> {
      const headers = { ...auth, ...(body === undefined ? {} : { "content-type": "application/json" }) };
      const r = await fetch(`${origin}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
      const text = await r.text();
      expect(r.status, `${method} ${path}: ${text}`).toBeLessThan(300);
      return (text ? JSON.parse(text) : null) as T;
    },
    async send(method: string, path: string, body?: unknown): Promise<{ status: number; body: { error?: { code?: string } } & Record<string, unknown> }> {
      const headers = { ...auth, ...(body === undefined ? {} : { "content-type": "application/json" }) };
      const r = await fetch(`${origin}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
      const text = await r.text();
      return { status: r.status, body: text ? JSON.parse(text) : {} };
    },
    async bytes(path: string): Promise<{ status: number; type: string | null; bytes: Uint8Array }> {
      const r = await fetch(`${origin}${path}`, { headers: auth });
      return { status: r.status, type: r.headers.get("content-type"), bytes: new Uint8Array(await r.arrayBuffer()) };
    },
  };
}

/** A user `user` of the deployer's site, signed in to the site (not to
 * Clax) in a browser of their own, on its page with the overlay loaded.
 * Every CSP violation the page reports is kept in `window.cspViolations`
 * as its directive, blocked URI and sample (JSON). */
export async function reporterOn(browser: Browser, site: DeployerSite, user: string): Promise<Page> {
  const page = await (await browser.newContext()).newPage();
  await page.addInitScript(() => {
    (window as unknown as { cspViolations: string[] }).cspViolations = [];
    document.addEventListener("securitypolicyviolation", e => {
      (window as unknown as { cspViolations: string[] }).cspViolations.push(`${e.violatedDirective} ${e.blockedURI} ${JSON.stringify(e.sample)}`);
    });
  });
  await page.goto(`${site.origin}/login?user=${user}`);
  await page.waitForURL(`${site.origin}/`);
  await expect(page.locator(".report-btn")).toBeVisible();
  return page;
}

export const cspViolations = (p: Page) => p.evaluate(() => (window as unknown as { cspViolations: string[] }).cspViolations);

/** "Your reports" in `page`'s overlay, opened afresh (opening loads the
 * reporter's reports at once; the panel's own poll is 15 s). */
export async function openReports(page: Page) {
  const btn = page.locator(".reports-btn");
  if ((await btn.getAttribute("aria-expanded")) === "true") await btn.click();
  await expect(btn).toHaveAttribute("aria-expanded", "false");
  await btn.click();
  const panel = page.getByRole("region", { name: "Your reports" });
  await expect(panel).toBeVisible();
  await expect(panel.getByText("Loading…")).toHaveCount(0);
  return panel;
}

/** A queue for `site`'s origin, reporters seeing their own reports, made
 * by `owner` on the Queues page, with a service grant handed to the site. */
export async function queueWithGrant(owner: Page, origin: string, site: DeployerSite): Promise<string> {
  await owner.goto(`${origin}/queues`);
  await owner.getByRole("button", { name: "New queue" }).click();
  const form = owner.locator("form.qform");
  await form.getByLabel("Name", { exact: true }).fill("Leave app reports");
  await form.getByLabel("Origins, one per line").fill(site.origin);
  await form.getByLabel("Their own reports").check();
  await form.getByRole("button", { name: "Create queue" }).click();
  const card = owner.locator("li.qcard", { has: owner.getByRole("heading", { name: "Leave app reports" }) });
  await expect(card).toBeVisible();
  const qid = (await card.getAttribute("data-id"))!;
  await card.getByRole("button", { name: "Manage" }).click();
  await card.getByLabel("Name", { exact: true }).fill("leave app backend");
  await card.getByRole("button", { name: "Create service grant" }).click();
  const secret = card.getByRole("region", { name: "New service grant" });
  await expect(secret).toBeVisible();
  const token = (await secret.locator("code").nth(0).textContent())!.trim();
  const privateKey = (await secret.locator("code").nth(1).textContent())!.trim();
  await secret.getByRole("button", { name: "Done" }).click();
  site.grant({ queue: qid, token, privateKey });
  return qid;
}

/** Tells `page`'s overlay the page has focus again, which asks for the badge at once. */
export const refocus = (page: Page) => page.evaluate(() => { window.dispatchEvent(new Event("focus")); });

/** An agent token of the owner's (`cxa_…`), made in the owner's signed-in browser. */
export async function agentTokenOf(owner: Page, label = "triage-laptop"): Promise<string> {
  return owner.evaluate(async l => {
    const r = await fetch("/api/hosted/agent-tokens", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ label: l }) });
    if (!r.ok) throw new Error(`agent token: ${r.status} ${await r.text()}`);
    return (await r.json()).token as string;
  }, label);
}
