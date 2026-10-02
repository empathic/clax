// The states every UI task photographs (light and dark, desktop and phone),
// on one seeded scratch daemon. Tasks append scenes as they add UI.
import type { Cookie, Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { api, contentFrame, postThread, publishAs, publishNext, registerSession, seenOf, setWorking } from "./fixtures";

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

/** The gallery with a card that needs the viewer's eyes: a viewer named
 * "alex" posts a thread on the seeded artifact, views v1, and the claude
 * session publishes v2 addressing that thread. Done once per run, which
 * leaves the seeded artifact at v2 for the scenes after it; each later
 * gallery shot takes the same viewer's cookie, so every shot shows one
 * state. Ends on the gallery. */
let eyes: Cookie[] | null = null;
async function needsEyes(page: Page, s: Seeded): Promise<void> {
  if (eyes) {
    await page.context().addCookies(eyes);
  } else {
    await name(page, "alex");
    const tid = await page.evaluate(async aid => {
      const f = new FormData();
      f.set("anchor", JSON.stringify({ kind: "element", selector: "#tgt-tbl", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
      f.set("body", "Add the week before, so the change shows.");
      f.set("version", "1");
      return (await (await fetch(`/api/artifacts/${aid}/threads`, { method: "POST", body: f })).json()).thread.id as string;
    }, s.aid);
    await page.goto(`${s.base}/a/${s.aid}`);
    await contentFrame(page, s.aid, 1);
    for (let i = 0; i < 50 && (await seenOf(page, s.aid)) !== 1; i++) await page.waitForTimeout(100);
    await publishNext(s.base, s.token, s.sid, s.aid, 1, { addresses: [tid] });
    eyes = await page.context().cookies();
  }
  await page.goto(`${s.base}/`);
  await page.locator(".grp.needs .card").waitFor();
}

const SEEDED = [
  ["Is p95 measured at the edge or at the app server? Say which in the label.", "#tgt-p95"],
  ["Mark the deploy on the chart itself.", "#tgt-chart"],
  ["Sort by p95, worst first.", "#tgt-tbl"],
] as const;

/** An artifact at v2 that addressed every thread, for the changelog scenes.
 * Each scene gets its own (its viewer is new), so the menu reads `v2 of 2`:
 * the sample report at v1 with the seeded threads, a thread by this page's
 * viewer, named "alex", sent to the agent and answered, then v2 with the note
 * `Two columns; units in ms`, after the viewer saw v1. Ends on v2's view. */
async function changelog(page: Page, s: Seeded): Promise<void> {
  const { artifact } = await publishAs(s.base, s.token, s.sid, "Checkout latency, week 39", { "index.html": REPORT });
  const ids: string[] = [];
  for (const [body, selector] of SEEDED) ids.push((await postThread(s.base, artifact.id, body, selector)).id);
  await page.goto(`${s.base}/a/${artifact.id}`);
  await contentFrame(page, artifact.id, 1);
  await name(page, "alex");
  const mine = await page.evaluate(async aid => {
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "#tgt-chart", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", "Put the two charts side by side, two columns.");
    f.set("version", "1");
    return (await (await fetch(`/api/artifacts/${aid}/threads`, { method: "POST", body: f })).json()).thread.id as string;
  }, artifact.id);
  const sent = await fetch(`${s.base}/api/artifacts/${artifact.id}/threads/${mine}/send`, { method: "POST", headers: { origin: s.base } });
  if (!sent.ok) throw new Error(`send: ${sent.status}`);
  await api(s.base, s.token, `/api/artifacts/${artifact.id}/threads/${mine}/comments`, { method: "POST", session: s.sid, body: JSON.stringify({ body: "Two columns now, and every duration in ms.", author_kind: "agent" }) });
  for (let i = 0; i < 50 && (await seenOf(page, artifact.id)) !== 1; i++) await page.waitForTimeout(100);
  await publishNext(s.base, s.token, s.sid, artifact.id, 1, { note: "Two columns; units in ms", addresses: [mine, ...ids] });
  await page.reload();
  await contentFrame(page, artifact.id, 2);
  await page.locator(".thread-pin").first().waitFor();
}

export const SCENES: Scene[] = [
  // The gallery with a card that needs the viewer's eyes, above everything else.
  { name: "gallery", path: () => "/", prepare: needsEyes },
  { name: "view", path: s => `/a/${s.aid}` },
  { name: "comment", path: s => `/a/${s.aid}`, prepare: async page => { await page.getByRole("button", { name: /^Comment/ }).click(); } },
  { name: "keys", path: s => `/a/${s.aid}`, prepare: async page => { await page.locator("body").press("Shift+?"); await page.getByRole("dialog", { name: "Keyboard shortcuts" }).waitFor(); } },
  { name: "threads", path: s => `/a/${s.aid}`, prepare: async page => {
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    await page.locator(".thread-card").first().click();
  } },
  // The claude session works on the artifact and on its first thread: the
  // roster's solid token, the summary, the sweep, the strip with its haiku,
  // the split pin and the card's marker.
  { name: "working", path: s => `/a/${s.aid}`, prepare: async (page, s) => {
    await setWorking(s.base, s.token, s.sid, s.aid, { thread_ids: [s.threads[0]] });
    await page.reload();
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    await page.locator(".strip .hk").waitFor();
  } },
  // The gallery while the claude session works: the card's chip and roster.
  { name: "working-gallery", path: () => "/", prepare: async (page, s) => {
    await setWorking(s.base, s.token, s.sid, s.aid, { thread_ids: [s.threads[0]] });
    await page.reload();
    await page.locator(".chip.ag").waitFor();
  } },
  // A returning viewer at v2, which addressed their thread: the dot, the
  // summary line, the Addressed group, the agent's reply, the history and
  // the addressed pins.
  { name: "changelog", path: () => "/", prepare: async (page, s) => {
    await changelog(page, s);
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    await page.locator(".section-addressed .thread-card").first().waitFor();
  } },
  // The version menu open over the same state; at phone width, where the
  // version button is hidden, the more menu's Versions item opens it as a sheet.
  { name: "versions", path: () => "/", prepare: async (page, s) => {
    await changelog(page, s);
    const button = page.getByRole("button", { name: /^Version 2 of 2/ });
    if (await button.isVisible()) await button.click();
    else {
      await page.getByRole("button", { name: "More", exact: true }).click();
      await page.getByRole("menuitem", { name: "Versions" }).click();
    }
    await page.getByRole("dialog", { name: "Versions" }).locator(".vrow").first().waitFor();
  } },
];
