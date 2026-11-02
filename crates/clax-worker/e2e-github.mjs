// GitHub sign-in on the Worker, end to end (`node e2e-github.mjs
// [--no-build]`): builds the Worker (the web UI must be built,
// scripts/build-web.sh), serves a fake GitHub (authorize, token exchange,
// `/user`) from this process, starts wrangler with GITHUB_CLIENT_ID,
// GITHUB_CLIENT_SECRET, OWNER_GITHUB and the GitHub bases pointed at the
// fake, and in a browser:
//
// - setup makes the owner under a placeholder name, as the instance's
//   first deploy did;
// - "Sign in with GitHub" as the owner's login signs in as that owner,
//   renamed from GitHub;
// - another account signs up as a person of their own, whose People shows
//   nothing of the owner's and who cannot read the owner's artifact;
// - the owner's People lists that person's GitHub login.
//
// The token exchange and `GET /user` are the Worker's outbound `fetch`.

import { createServer } from "node:http";
import { mkdirSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { HERE, STATE, build, freePort, pruneRuns, start, writeConfig } from "./harness.mjs";

const require = createRequire(join(HERE, "../../web/package.json"));
const { chromium } = require("@playwright/test");

const ROOT = join(STATE, "e2e-github");
const SECRET = "e2e setup secret " + Math.random().toString(36).slice(2);
const TOKEN = "e2e-owner-token-" + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);

function check(ok, what, detail) {
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

/** The fake GitHub: `next` is the account the next authorize signs in. */
function fakeGitHub() {
  const users = {
    1: { id: 1, login: "TheOwner", name: "Alex K", avatar_url: null },
    2: { id: 2, login: "grace", name: "Grace Hopper", avatar_url: null },
  };
  const state = { next: 1, exchanges: [] };
  const server = createServer((req, res) => {
    const u = new URL(req.url, "http://x");
    if (u.pathname === "/login/oauth/authorize") {
      const to = new URL(u.searchParams.get("redirect_uri"));
      to.searchParams.set("code", `code-${state.next}`);
      to.searchParams.set("state", u.searchParams.get("state"));
      res.writeHead(302, { location: to.href });
      res.end();
      return;
    }
    if (u.pathname === "/login/oauth/access_token" && req.method === "POST") {
      let b = "";
      req.on("data", (c) => (b += c));
      req.on("end", () => {
        const f = new URLSearchParams(b);
        state.exchanges.push(Object.fromEntries(f));
        res.setHeader("content-type", "application/json");
        const ok = f.get("client_secret") === "e2e-client-secret" && (f.get("code_verifier") ?? "").length === 43;
        res.end(JSON.stringify(ok ? { access_token: `tok-${f.get("code").slice(5)}` } : { error: "bad_verification_code" }));
      });
      return;
    }
    if (u.pathname === "/user") {
      const id = (req.headers.authorization ?? "").replace("Bearer tok-", "");
      if (!users[id] || !req.headers["user-agent"]) {
        res.writeHead(401);
        res.end();
        return;
      }
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(users[id]));
      return;
    }
    res.writeHead(404);
    res.end();
  });
  return { server, state };
}

async function main() {
  await build();
  pruneRuns(ROOT, 3);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const gh = fakeGitHub();
  const ghPort = await freePort();
  await new Promise((r) => gh.server.listen(ghPort, "127.0.0.1", r));
  const ghBase = `http://127.0.0.1:${ghPort}`;
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = writeConfig(dir, {
    HOSTED: "1",
    PUBLIC_ORIGIN: origin,
    SITE_DOMAIN: "localhost",
    SETUP_SECRET: SECRET,
    CLAX_TOKEN: TOKEN,
    GITHUB_CLIENT_ID: "e2e-client",
    GITHUB_CLIENT_SECRET: "e2e-client-secret",
    OWNER_GITHUB: "theowner",
    GITHUB_OAUTH_BASE: ghBase,
    GITHUB_API_BASE: ghBase,
  });
  const browser = await chromium.launch();
  let w;
  const summary = { ok: false, dir };
  try {
    w = await start(dir, config, 1, port);
    const owner = await browser.newContext();
    const op = await owner.newPage();
    const setup = await op.request.post(`${origin}/api/hosted/setup`, {
      headers: { origin },
      data: { secret: SECRET, name: "Placeholder" },
    });
    const made = await setup.json();
    check(setup.ok() && made.person.owner, "setup makes the owner under a placeholder name", made);
    await op.request.post(`${origin}/api/hosted/signout`, { headers: { origin } });

    gh.state.next = 1;
    await op.goto(`${origin}/signin`);
    await op.getByTestId("github-signin").click();
    await op.waitForURL(`${origin}/`);
    let me = await (await op.request.get(`${origin}/api/hosted/me`)).json();
    check(
      me.person?.owner === true && me.person.public_id === made.person.public_id && me.person.display_name === "Alex K",
      "the owner's GitHub login signs in as the owner, renamed from GitHub",
      me,
    );
    check(gh.state.exchanges.length === 1 && gh.state.exchanges[0].client_id === "e2e-client", "the Worker exchanged the code with GitHub", gh.state.exchanges);
    const pub = await (await fetch(`${origin}/api/artifacts`, {
      method: "POST",
      headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ title: "Owner's plan", files: { "index.html": { content: "<h1>plan</h1>", encoding: "utf8" } } }),
    })).json();
    const aid = pub.artifact.id;

    gh.state.next = 2;
    const member = await browser.newContext();
    const mp = await member.newPage();
    await mp.goto(`${origin}/signin`);
    await mp.getByTestId("github-signin").click();
    await mp.waitForURL(`${origin}/`);
    me = await (await mp.request.get(`${origin}/api/hosted/me`)).json();
    check(me.person && !me.person.owner && me.person.display_name === "Grace Hopper", "another account signs up as a person of their own", me);
    const theirs = await mp.request.get(`${origin}/api/artifacts/${aid}`);
    check(theirs.status() === 404, "the new person cannot read the owner's artifact", theirs.status());
    const list = await (await mp.request.get(`${origin}/api/artifacts`)).json();
    // Their gallery may hold their own seeded quickstart, never the owner's page.
    check(!(list.artifacts ?? []).some((a) => a.id === aid), "the new person's gallery holds nothing of the owner's", list);

    await op.goto(`${origin}/people`);
    await op.getByTestId("github-login").first().waitFor();
    const logins = await op.getByTestId("github-login").allTextContents();
    check(logins.includes("@grace"), "the owner's People shows the person's GitHub login", logins);

    summary.ok = true;
    console.log(JSON.stringify(summary));
  } finally {
    await browser.close().catch(() => {});
    if (w) await w.stop();
    gh.server.close();
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
