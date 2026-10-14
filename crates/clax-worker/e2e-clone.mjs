// The shell's Clone button and dialog on the Worker as a hosted Clax
// (HOSTED=1), end to end under `wrangler dev --local`: builds the Worker
// (./build.sh; the web UI must be built, scripts/build-web.sh), starts
// wrangler with fresh state under target/e2e-clone/, `http://localhost:<port>`
// as the public origin and `http://127.0.0.1:<port>` as the content origin,
// publishes an artifact with the owner token (the CLI's credential), and
// signs the first person, Ada, in with the setup secret. Her browser opens
// the artifact at 1280 and 390 pixels wide, light and dark, signed in by its
// session cookie alone. Wide, the top bar's Clone opens the dialog; at phone
// width the more menu's Clone with git does. Each makes a read token, and
// the dialog's git clone command, run as shown, clones the artifact. Sam,
// a member Ada invites, may only read Ada's artifact; a browser with no
// session gets no token. Screenshots of each go to
// target/e2e-clone/<run>/shots (or CLAX_E2E_SHOTS). Prints one line per
// check; exits non-zero on the first failure.
//
// `node e2e-clone.mjs --no-build` skips the build.

import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { HERE, build, freePort, pruneRuns, start, writeConfig } from "./harness.mjs";

const require = createRequire(join(HERE, "../../web/package.json"));
const { chromium } = require("@playwright/test");

const ROOT = join(HERE, "target/e2e-clone");
const TOKEN = "clone-owner-token-" + Math.random().toString(36).slice(2);
const SECRET = "clone setup secret " + Math.random().toString(36).slice(2);
const LIMIT = 15_000;

function check(ok, what, detail) {
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

async function api(base, path, init = {}) {
  const res = await fetch(`${base}${path}`, {
    ...init,
    headers: { "content-type": "application/json", authorization: `Bearer ${TOKEN}`, ...(init.headers ?? {}) },
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${init.method ?? "GET"} ${path}: ${res.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

/** Ada's browser state, signed in with the setup secret; and Sam's, a
 * member she invites. */
async function signIn(browser, origin) {
  const ctx = await browser.newContext();
  const ada = await ctx.newPage();
  await ada.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
  await ada.getByLabel("Your name").fill("Ada");
  await ada.getByRole("button", { name: "Create the first person" }).click();
  await ada.waitForURL(`${origin}/`);
  await ada.goto(`${origin}/people`);
  await ada.getByRole("button", { name: "Invite a person" }).click();
  await ada.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
  await ada.getByRole("button", { name: "Create invite" }).click();
  const join = (await ada.locator(".invite code").textContent()) ?? "";
  const adaState = await ctx.storageState();
  await ctx.close();
  const sctx = await browser.newContext();
  const sam = await sctx.newPage();
  await sam.goto(join);
  await sam.getByRole("button", { name: "Join as Sam" }).click();
  await sam.waitForURL(`${origin}/`);
  const samState = await sctx.storageState();
  await sctx.close();
  return { adaState, samState };
}

/** Runs `command` with `sh` in `cwd`, with no git configuration of the person's. */
function sh(cwd, command, home) {
  const r = spawnSync("sh", ["-c", command], {
    cwd,
    encoding: "utf8",
    timeout: 120_000,
    env: { PATH: process.env.PATH, HOME: home, GIT_CONFIG_NOSYSTEM: "1", GIT_TERMINAL_PROMPT: "0" },
  });
  return { status: r.status, stderr: r.stderr };
}

async function main() {
  await build();
  pruneRuns(ROOT, 3);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  const shots = process.env.CLAX_E2E_SHOTS || join(dir, "shots");
  const home = join(dir, "home");
  mkdirSync(shots, { recursive: true });
  mkdirSync(home, { recursive: true });
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = writeConfig(dir, {
    HOSTED: "1",
    PUBLIC_ORIGIN: origin,
    CONTENT_ORIGIN: `http://127.0.0.1:${port}`,
    SETUP_SECRET: SECRET,
    CLAX_TOKEN: TOKEN,
  });
  const w = await start(dir, config, 1, port);
  const browser = await chromium.launch();
  try {
    const page = "<title>Tide tables</title><main><h2 id=h>Tide tables</h2><p>High water at 06:12.</p></main>";
    const { artifact } = await api(origin, "/api/artifacts", {
      method: "POST",
      body: JSON.stringify({ title: "Tide tables", files: { "index.html": { content: page, encoding: "utf8" } } }),
    });
    const id = artifact.id;
    const { adaState, samState } = await signIn(browser, origin);
    check(true, "Ada sets up with the secret and invites Sam, who joins");
    for (const scheme of ["light", "dark"]) {
      for (const [width, height] of [[1280, 800], [390, 844]]) {
        const label = `${width} ${scheme}`;
        const ctx = await browser.newContext({ viewport: { width, height }, colorScheme: scheme, storageState: adaState });
        const p = await ctx.newPage();
        p.on("pageerror", (e) => console.log(`     [${label} error] ${e.message}`));
        await p.goto(`${origin}/a/${id}`);
        if (width > 700) {
          const clone = p.locator("button.clone");
          await clone.waitFor({ timeout: LIMIT });
          check(await clone.isVisible(), `${label}: the top bar shows Clone`, null);
          await clone.click();
        } else {
          check(!(await p.locator("button.clone").isVisible()), `${label}: the bar keeps no Clone button at phone width`, null);
          const more = p.getByRole("button", { name: "More" });
          await more.waitFor({ timeout: LIMIT });
          const item = p.getByRole("menuitem", { name: "Clone with git…" });
          // The menu lists Clone once the options are in.
          const end = Date.now() + LIMIT;
          for (;;) {
            await more.click();
            if (await item.isVisible()) break;
            await p.keyboard.press("Escape");
            if (Date.now() > end) throw new Error(`FAIL ${label}: the more menu lists no Clone with git`);
            await p.waitForTimeout(100);
          }
          await item.click();
        }
        const dialog = p.getByRole("dialog", { name: "Clone with git or jj" });
        await dialog.waitFor({ timeout: LIMIT });
        const radios = await dialog.locator("input[type=radio]").evaluateAll((els) => els.map((e) => e.value));
        check(radios.join(",") === "read,write", `${label}: Ada, signed in, may choose read or write`, radios);
        await shot(p, shots, `clone-${width}-${scheme}-open`);
        await dialog.getByRole("button", { name: "Make a token" }).click();
        const git = dialog.locator("pre[aria-label=git]");
        await git.waitFor({ timeout: LIMIT });
        const command = (await git.textContent()) ?? "";
        check(command.startsWith('git -c http.extraHeader="Authorization: Bearer cxm_read_') && command.endsWith(`/git/a-${id}.git`), `${label}: the dialog shows the git clone command`, command);
        const jj = (await dialog.locator("pre[aria-label=jj]").textContent()) ?? "";
        check(jj.includes(" jj git clone "), `${label}: and the jj one`, jj);
        const meta = (await dialog.locator(".meta").textContent()) ?? "";
        check(/expires in about (59|60) min/.test(meta), `${label}: and when the token expires`, meta);
        const overflow = await p.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
        check(overflow <= 0, `${label}: nothing scrolls sideways`, overflow);
        await shot(p, shots, `clone-${width}-${scheme}-token`);
        check(
          (await p.getByText("Comment mode could not load").count()) === 0,
          `${label}: the page shows no comment-mode failure`,
          null,
        );
        const out = `clone-${width}-${scheme}`;
        const r = sh(home, `${command} ${out}`, home);
        check(r.status === 0 && readFileSync(join(home, out, "index.html"), "utf8") === page, `${label}: the shown git command clones the artifact`, r.stderr);
        check(
          await p.evaluate(() => !!document.activeElement?.closest("[role=dialog]")),
          `${label}: focus stays in the dialog once the token is shown`,
          await p.evaluate(() => document.activeElement?.tagName),
        );
        await p.keyboard.press("Escape");
        await dialog.waitFor({ state: "detached", timeout: LIMIT });
        await ctx.close();
      }
    }
    // Sam, a member: Ada's artifact is his to read, not to push to.
    const sctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, storageState: samState });
    const sp = await sctx.newPage();
    const offered = sp.waitForResponse((r) => r.url().endsWith(`/api/artifacts/${id}/git-token`), { timeout: LIMIT });
    await sp.goto(`${origin}/a/${id}`);
    const options = await (await offered).json();
    check(options.available === true && options.scopes.join(",") === "read", "Sam is offered a read token only", options);
    await sp.locator("button.clone").click();
    const sd = sp.getByRole("dialog", { name: "Clone with git or jj" });
    await sd.waitFor({ timeout: LIMIT });
    const sr = await sd.locator("input[type=radio]").evaluateAll((els) => els.map((e) => e.value));
    check(sr.join(",") === "read", "and his dialog offers read alone", sr);
    const pushed = await sp.evaluate(async (aid) => {
      const r = await fetch(`/api/artifacts/${aid}/git-token`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ scope: "write" }) });
      return { status: r.status, code: (await r.json().catch(() => ({})))?.error?.code ?? null };
    }, id);
    check(pushed.status === 403 && pushed.code === "not_permitted", "a write token is refused him", pushed);
    await sctx.close();
    // No session: no token, whatever the request says.
    const anon = await fetch(`${origin}/api/artifacts/${id}/git-token`, {
      method: "POST",
      headers: { "content-type": "application/json", origin },
      body: JSON.stringify({ scope: "read" }),
    });
    check([401, 403, 404].includes(anon.status), "a request with no session gets no token", anon.status);
    console.log(JSON.stringify({ ok: true, shots }));
  } finally {
    await browser.close();
    await w.stop();
  }
}

async function shot(page, dir, name) {
  await page.screenshot({ path: join(dir, `${name}.png`) });
}

main().catch((e) => {
  console.error(e.message ?? e);
  process.exit(1);
});
