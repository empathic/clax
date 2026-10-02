// The states every UI task photographs (light and dark, desktop and phone),
// on one seeded scratch daemon. Tasks append scenes as they add UI.
import type { Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { api, postThread, publishAs, registerSession } from "./fixtures";

export type Seeded = { base: string; token: string; aid: string; sid: string; threads: string[] };
export type Scene = { name: string; path(s: Seeded): string; prepare?(page: Page, s: Seeded): Promise<void> };

const REPORT = readFileSync(new URL("./pages/sample-report.html", import.meta.url), "utf8");

/** One artifact by a claude session with three threads (the agent has
 * answered the second), and two quieter artifacts: a pinned probe from the
 * command line and a codex checklist at its tenth version. The threads are posted without a viewer cookie, so each one's
 * author is an unnamed viewer and shows as "Viewer"; the gallery scene's
 * name is its own browser's viewer, not theirs. */
export async function seed(base: string, token: string): Promise<Seeded> {
  const s = await registerSession(base, token, "claude", "shots");
  const { artifact } = await publishAs(base, token, s.id, "Checkout latency, week 39", { "index.html": REPORT });
  const threads: string[] = [];
  // Each thread is anchored to an element of the sample page: the p95 tile, the chart, the table.
  for (const [body, selector] of [
    ["Is p95 measured at the edge or at the app server? Say which in the label.", "#tgt-p95"],
    ["Mark the deploy on the chart itself.", "#tgt-chart"],
    ["Sort by p95, worst first.", "#tgt-tbl"],
  ]) {
    threads.push((await postThread(base, artifact.id, body, selector)).id);
  }
  // The viewer sends the chart thread and the agent answers it, so a card
  // shows an agent's message.
  const sent = await fetch(`${base}/api/artifacts/${artifact.id}/threads/${threads[1]}/send`, { method: "POST", headers: { origin: base } });
  if (!sent.ok) throw new Error(`send: ${sent.status}`);
  await api(base, token, `/api/artifacts/${artifact.id}/threads/${threads[1]}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: "Added a dashed line at the deploy, labelled with its time.", author_kind: "agent" }) });
  const other = await registerSession(base, token, "codex", "shots-codex");
  // The checklist reaches its tenth version, so the gallery shows the rally
  // chip; the probe, published first, is pinned, so it leads the gallery.
  const probe = (await api(base, token, "/api/artifacts", { method: "POST", body: JSON.stringify({ title: "Permissions probe", files: { "index.html": { content: "<main><h2>Probe</h2></main>", encoding: "utf8" } } }) })) as { artifact: { id: string } };
  await api(base, token, `/api/artifacts/${probe.artifact.id}`, { method: "PATCH", body: JSON.stringify({ pinned: true }) });
  const list = await publishAs(base, token, other.id, "Onboarding checklist", { "index.html": "<main><h2>First week</h2><ul><li>Laptop</li><li>Access</li></ul></main>" });
  for (let v = 2; v <= 10; v++) {
    await publishAs(base, token, other.id, "Onboarding checklist", { "index.html": `<main><h2>First week</h2><ul><li>Laptop</li><li>Access</li><li>Step ${v}</li></ul></main>` }, v - 1, list.artifact.id);
  }
  return { base, token, aid: artifact.id, sid: s.id, threads };
}

const name = async (page: Page, who: string) => {
  await page.evaluate(n => fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: n }) }), who);
};

export const SCENES: Scene[] = [
  { name: "gallery", path: () => "/", prepare: async page => { await name(page, "alex"); await page.reload(); } },
  { name: "view", path: s => `/a/${s.aid}` },
  { name: "comment", path: s => `/a/${s.aid}`, prepare: async page => { await page.getByRole("button", { name: /^Comment/ }).click(); } },
  { name: "keys", path: s => `/a/${s.aid}`, prepare: async page => { await page.locator("body").press("Shift+?"); await page.getByRole("dialog", { name: "Keyboard shortcuts" }).waitFor(); } },
  { name: "threads", path: s => `/a/${s.aid}`, prepare: async page => {
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    await page.locator(".thread-card").first().click();
  } },
];
