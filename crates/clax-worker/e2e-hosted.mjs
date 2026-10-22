// The Worker as a hosted Clax (HOSTED=1), end to end under `wrangler dev
// --local`, in four browsers and an agent: builds the Worker (./build.sh;
// the web UI must be built, scripts/build-web.sh), starts wrangler with
// fresh state under target/e2e-hosted/, `http://localhost:<port>` as the
// public origin and `localhost` as the site domain (each artifact's site at
// `http://<ID>.localhost:<port>`), and one people seat. It checks that
//
// - a request body over 96 MiB, over 16 MiB, or over 256 KiB with no
//   credential, declared or chunked, is refused 413;
// - the first person sets up with the secret, invites Sam, who joins in a
//   second browser in a space of their own, and Sam signs a third browser
//   in with a sign-in link;
// - Sam mints an agent token in the browser, and an agent with it reaches
//   `/mcp`, publishes, and is refused what Sam's rights do not allow;
// - the owner shares a page with Sam from its Share dialog, and it
//   reaches Sam's gallery; the owner sends a thread on it to Sam's agent;
//   the approval card reaches
//   Sam's open inbox live, over the WebSocket stream; Sam allows the
//   thread there and the agent then reads the comment; Sam narrows the
//   agent's session and it is refused what the narrowing leaves out;
// - a page's site is served at the root of its own host, where no
//   path-first route of the Worker (`/git/`, `/_diag/`, `/_dev/repos/`,
//   `/do/`, `/mcp`, the test routes) is reached;
// - a db document written through the API lives in its artifact's own
//   object and not in the instance's database (`GET /_worker/app-data`),
//   and goes when the artifact is deleted;
// - two artifacts' sites keep their localStorage apart in one browser;
// - with the access check (`/site-signin` served; skipped, saying so,
//   while it answers 404): a private site is refused signed out and served
//   to the owner's browser through the handshake; a site opened to the
//   world is served signed out until it is closed again (skipped while
//   `PATCH site_access` is not served); an old keyed asset URL serves no
//   bytes, at most a redirect to its artifact's site;
// - a guest with the link of a page shared for comments names themselves
//   and comments, and the owner sees it live;
// - the owner's and the guest's pointers reach each other over the live
//   socket, through the bridge in the page's frame;
// - Sam, beyond the one seat, is a spectator: their writes are refused;
// - after a simulated wake (POST /_worker/hibernate) the guest is still
//   seated and Sam still a spectator before any browser subscribes again,
//   and comments still arrive live.
//
// Prints one line per check and a JSON summary; exits non-zero on the first
// failure. `node e2e-hosted.mjs --no-build` skips the build;
// CLAX_E2E_SHOTS=<dir> saves screenshots there.

import { mkdirSync } from "node:fs";
import { request } from "node:http";
import { createRequire } from "node:module";
import { join } from "node:path";
import { HERE, build, freePort, pruneRuns, start, writeConfig } from "./harness.mjs";

const require = createRequire(join(HERE, "../../web/package.json"));
const { chromium } = require("@playwright/test");

const ROOT = join(HERE, "target/e2e-hosted");
const TOKEN = "e2e-owner-token-" + Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
const SECRET = "e2e setup secret " + Math.random().toString(36).slice(2);
const LIMIT = 20_000;
const SHOTS = process.env.CLAX_E2E_SHOTS ?? "";

function check(ok, what, detail) {
  if (!ok) throw new Error(`FAIL ${what}: ${JSON.stringify(detail)}`);
  console.log(`ok   ${what}`);
}

async function until(what, f, ms = LIMIT) {
  const end = Date.now() + ms;
  let last;
  for (;;) {
    try {
      last = await f();
      if (last) return last;
    } catch (e) {
      last = e instanceof Error ? e.message : e;
    }
    if (Date.now() > end) throw new Error(`FAIL ${what}: ${JSON.stringify(last)}`);
    await new Promise((r) => setTimeout(r, 150));
  }
}

/** The owner token's API call (the CLI's credential on a hosted instance). */
async function api(base, path, init = {}) {
  const res = await fetch(`${base}${path}`, {
    ...init,
    headers: { "content-type": "application/json", authorization: `Bearer ${TOKEN}`, ...(init.headers ?? {}) },
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${init.method ?? "GET"} ${path}: ${res.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

/** Each agent token's MCP session: the `Mcp-Session-Id` its `initialize` was answered with. */
const mcpSessions = new Map();

/** One JSON-RPC call on `/mcp` with agent token `token`, in the session its `initialize` opened. */
async function mcp(base, token, method, params, id = 1) {
  const headers = { "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${token}` };
  const session = mcpSessions.get(token);
  if (session && method !== "initialize") headers["mcp-session-id"] = session;
  const res = await fetch(`${base}/mcp`, { method: "POST", headers, body: JSON.stringify({ jsonrpc: "2.0", id, method, params }) });
  const text = await res.text();
  if (!res.ok) throw new Error(`/mcp ${method}: ${res.status} ${text}`);
  if (method === "initialize") {
    const sid = res.headers.get("mcp-session-id");
    if (!sid) throw new Error("/mcp initialize answered no Mcp-Session-Id");
    mcpSessions.set(token, sid);
  }
  return JSON.parse(text);
}

/** A tool call's result: whether it is an error, and its text parsed when it is JSON. */
async function tool(base, token, name, args) {
  const r = await mcp(base, token, "tools/call", { name, arguments: args });
  if (r.error) return { error: true, text: JSON.stringify(r.error) };
  const text = (r.result?.content ?? []).map((c) => c.text ?? "").join("");
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {
    // a message, not JSON
  }
  return { error: r.result?.isError === true, text, json };
}

/** The status of a POST to `path` whose body is `mib` MiB: declared so by
 * its `Content-Length`, or chunked with no length, written 1 MiB at a time
 * until the answer comes; `extra` headers beside. */
function oversized(base, path, chunked, mib = 97, extra = {}) {
  const total = mib * 1024 * 1024;
  const chunk = Buffer.alloc(1024 * 1024, 0x20);
  return new Promise((resolve, reject) => {
    const u = new URL(path, base);
    const headers = { "content-type": "application/json", origin: base, ...extra, ...(chunked ? {} : { "content-length": String(total) }) };
    const req = request({ host: u.hostname, port: u.port, path: u.pathname, method: "POST", headers }, (res) => {
      res.resume();
      req.destroy();
      resolve(res.statusCode);
    });
    req.on("error", (e) => (answered ? undefined : reject(e)));
    let answered = false;
    req.on("response", () => (answered = true));
    let sent = 0;
    const more = () => {
      while (!answered && sent < total) {
        sent += chunk.length;
        if (!req.write(chunk)) return void req.once("drain", more);
      }
      if (!answered) req.end();
    };
    more();
  });
}

async function newPage(browser, name) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const p = await ctx.newPage();
  p.on("pageerror", (e) => console.log(`     [${name} error] ${e.message}`));
  return p;
}

/** Records the frames every `/live` WebSocket of `page` receives. */
function watchLive(page) {
  const seen = { opened: 0, open: 0, frames: [] };
  page.on("websocket", (ws) => {
    if (!new URL(ws.url()).pathname.endsWith("/live")) return;
    seen.opened += 1;
    seen.open += 1;
    ws.on("close", () => (seen.open -= 1));
    ws.on("framereceived", (f) => seen.frames.push(String(f.payload)));
  });
  return seen;
}

async function threads(page) {
  const toggle = page.locator("button.threads");
  if ((await toggle.getAttribute("aria-pressed")) !== "true") await toggle.click();
  await page.locator(".new-comment-open").waitFor({ timeout: LIMIT });
}

async function setName(page, name) {
  await page.getByRole("button", { name: "People and agents" }).click();
  const field = page.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
  await field.fill(name);
  await field.press("Enter");
  await page.keyboard.press("Escape");
}

async function comment(page, text) {
  await threads(page);
  await page.locator(".new-comment-open").click();
  const composer = page.locator(".new-comment .composer");
  await composer.locator("textarea").fill(text);
  await composer.getByRole("button", { name: "Post comment" }).click();
  await composer.waitFor({ state: "detached", timeout: LIMIT });
}

/** A thread `p`'s browser posts on `aid` as the shell does: its status and error code. */
async function postThread(p, aid, body) {
  return p.evaluate(async ([id, text]) => {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "main", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    form.set("body", text);
    form.set("version", "1");
    const r = await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: form });
    const j = await r.json().catch(() => ({}));
    return { status: r.status, code: j?.error?.code ?? null };
  }, [aid, body]);
}

/** `p`'s browser's same-origin JSON call: its status and body. */
async function call(p, method, path, body) {
  return p.evaluate(async ([m, url, b]) => {
    const r = await fetch(url, { method: m, headers: { "content-type": "application/json" }, body: b === undefined ? undefined : JSON.stringify(b) });
    return { status: r.status, body: await r.json().catch(() => null) };
  }, [method, path, body]);
}

/** The frame of `id`'s version `n` in the artifact page: on the artifact's
 * site host, or the public origin's `/c/` address of the shell's sandboxed
 * mode. */
async function contentFrame(page, id, n) {
  const url = new RegExp(`^http://(${id}\\.localhost:\\d+/(_clax/v/${n}/)?|localhost:\\d+/c/${id}/v/${n}/)([?#].*)?$`);
  return until(`the frame of v${n}`, () => page.frame({ url }));
}

/** `GET <url>` without following a redirect: its status, `Location`, the
 * headers and the body's text. */
async function raw(url, headers = {}) {
  const r = await fetch(url, { redirect: "manual", headers });
  return { status: r.status, location: r.headers.get("location"), headers: r.headers, text: await r.text() };
}

/** Whether `r` (from `raw`) is the site's refusal, never a route's answer:
 * 404, or the access check's 401 `site_session_required`. */
function siteRefused(r) {
  if (r.status === 404) return true;
  if (r.status !== 401) return false;
  try {
    return JSON.parse(r.text)?.error?.code === "site_session_required";
  } catch {
    return false;
  }
}

/** Says why a step is skipped. */
function skip(what, why) {
  console.log(`skip ${what}: ${why}`);
}

async function shot(page, name) {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `${name}.png`) });
}

const PAGE = `<title>Tide tables</title><main style="font: 18px/1.6 system-ui; max-width: 640px; margin: 40px auto; padding: 0 16px">
<h2 id=h>Tide tables</h2><p id=p>High water at 06:12, low water at 12:30, and high again at 18:41.</p>
<section style="height: 240px">Chart</section></main>`;

async function main() {
  await build();
  pruneRuns(ROOT, 3);
  const dir = join(ROOT, `run-${Date.now()}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  // Each artifact's site host; browsers and Node send `*.localhost` to this machine.
  const siteOf = (aid) => `http://${aid}.localhost:${port}`;
  const config = writeConfig(dir, {
    HOSTED: "1",
    PUBLIC_ORIGIN: origin,
    SITE_DOMAIN: "localhost",
    SETUP_SECRET: SECRET,
    CLAX_TOKEN: TOKEN,
    CLAX_TEST_ROUTES: "1",
    CLAX_LIVE_PEOPLE_SEATS: "1",
    CLAX_LIVE_GUEST_SEATS: "1",
  });
  const browser = await chromium.launch();
  let w;
  const cleanup = async () => {
    await browser.close().catch(() => {});
    if (w) await w.stop();
  };
  for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => void cleanup().then(() => process.exit(130)));
  const summary = { ok: false, dir };
  const pages = [];
  try {
    w = await start(dir, config, 1, port);
    const { artifact } = await api(origin, "/api/artifacts", {
      method: "POST",
      body: JSON.stringify({ title: "Tide tables", files: { "index.html": { content: PAGE, encoding: "utf8" } } }),
    });
    const id = artifact.id;
    check(artifact.current_version === 1, "the owner token publishes on a hosted instance", artifact);
    const me = await fetch(`${origin}/api/hosted/me`).then((r) => r.json());
    check(me.setup_open === true && me.origin === origin, "the instance is hosted at its public origin, setup open", me);

    // A body over the largest any route takes, or than the isolate could
    // hold beside what it is made into (past 16 MiB with a credential, past
    // 256 KiB without one), is refused before it is read whole, and the
    // instance keeps answering. The Worker refuses a declared length; the
    // object, a chunked body once it is past the limit. Under `wrangler
    // dev` the request after an answer to a body still being sent can fail
    // (its proxy loses the connection), so one that may fail follows each.
    const settle = () => fetch(`${origin}/healthz`).catch(() => undefined);
    const bearer = { authorization: `Bearer ${TOKEN}` };
    const anonymous = await oversized(origin, "/api/hosted/setup", false, 1);
    check(anonymous === 413, "a body declared over 256 KiB with no credential is refused 413", anonymous);
    check((await oversized(origin, "/api/artifacts", false, 17, bearer)) === 413, "a body declared over 16 MiB is refused 413");
    check((await oversized(origin, "/api/hosted/setup", false)) === 413, "a body declared over 96 MiB is refused 413");
    const chunked16 = await oversized(origin, "/api/artifacts", true, 17, bearer);
    check(chunked16 === 413, "a chunked body past 16 MiB is refused 413", chunked16);
    await settle();
    const chunked96 = await oversized(origin, "/api/hosted/setup", true);
    check(chunked96 === 413, "a chunked body past 96 MiB is refused 413", chunked96);
    await settle();
    check((await fetch(`${origin}/healthz`)).ok, "the instance answers after the oversized bodies");

    // Setup, with the secret.
    const ada = await newPage(browser, "ada");
    pages.push(ada);
    const wrong = await fetch(`${origin}/api/hosted/setup`, {
      method: "POST",
      headers: { "content-type": "application/json", origin },
      body: JSON.stringify({ secret: "a guess", name: "Eve" }),
    });
    check(wrong.status === 404, "setup with a wrong secret is refused", wrong.status);
    await ada.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await ada.getByLabel("Your name").fill("Ada");
    await ada.getByRole("button", { name: "Create the first person" }).click();
    await ada.waitForURL(`${origin}/`);
    await ada.locator("a.card", { hasText: "Tide tables" }).waitFor({ timeout: LIMIT });
    check(true, "the first person sets up with the secret and sees the gallery");
    check((await fetch(`${origin}/setup`)).status === 404, "setup is closed once done");

    // An invite, joined in a second browser.
    await ada.locator("a.people-link").click();
    await ada.waitForURL(`${origin}/people`);
    await ada.getByRole("button", { name: "Invite a person" }).click();
    await ada.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
    await ada.getByRole("button", { name: "Create invite" }).click();
    const join = (await ada.locator(".invite code").textContent()) ?? "";
    check(join.startsWith(`${origin}/join/`), "an invite is a link on the public origin", join);
    const sam = await newPage(browser, "sam");
    pages.push(sam);
    await sam.goto(join);
    await sam.getByRole("button", { name: "Join as Sam" }).click();
    await sam.waitForURL(`${origin}/`);
    await sam.locator("main").waitFor({ timeout: LIMIT });
    check((await sam.locator("a.card", { hasText: "Tide tables" }).count()) === 0, "Sam joins with the invite in a second browser, in a space of their own");


    // A sign-in link for a third browser.
    await sam.goto(`${origin}/people`);
    await sam.getByRole("button", { name: "Sign in another device" }).click();
    const link = (await sam.locator(".hosted-me code").textContent()) ?? "";
    check(link.startsWith(`${origin}/signin/`), "a sign-in link is made", link);
    const laptop = await newPage(browser, "laptop");
    pages.push(laptop);
    await laptop.goto(link);
    await laptop.getByRole("button", { name: "Sign in as Sam" }).click();
    await laptop.waitForURL(`${origin}/`);
    await laptop.goto(`${origin}/people`);
    await laptop.locator(".hosted-me", { hasText: "Signed in as Sam" }).waitFor({ timeout: LIMIT });
    check(true, "the sign-in link signs a third browser in as Sam");

    // An agent token, minted in the browser, used by an agent on /mcp.
    await laptop.getByLabel("Machine").fill("sam-laptop");
    await laptop.getByRole("button", { name: "Create token" }).click();
    const cmd = (await laptop.locator(".minted code").first().textContent()) ?? "";
    const token = /Bearer (cxa_[A-Za-z0-9_-]+)/.exec(cmd)?.[1];
    check(token && cmd.includes(`${origin}/mcp`), "Sam mints an agent token whose command names the public /mcp", cmd);
    const init = await mcp(origin, token, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } });
    check(init.result?.serverInfo, "the agent token initializes /mcp", init);
    const listed = await mcp(origin, token, "tools/list", {}, 2);
    check((listed.result?.tools ?? []).some((t) => t.name === "publish"), "the agent lists the tools", listed.result?.tools?.length);
    const pub = await tool(origin, token, "publish", { html: "<title>Sam's notes</title><h1>Sam's notes</h1>" });
    const agentAid = pub.json?.id ?? pub.json?.artifact?.id ?? /\b([0-9a-z]{12})\b/.exec(pub.text)?.[1];
    check(!pub.error && agentAid, "the agent publishes with Sam's token", pub.text.slice(0, 300));
    await laptop.goto(`${origin}/`);
    await laptop.locator("a.card", { hasText: "Sam's notes" }).waitFor({ timeout: LIMIT });
    check(true, "Sam sees the agent's page in the gallery");
    const local = await tool(origin, token, "publish", { file_path: "/etc/hosts", title: "Hosts" });
    check(local.error && local.text.includes("invalid_args"), "the agent's tools read no file of the Worker's machine", local.text.slice(0, 300));
    const del = await tool(origin, token, "delete", { id });
    check(del.error, "the agent may not delete the owner's page (Sam's rights do not allow it)", del.text.slice(0, 300));
    const stillThere = await api(origin, `/api/artifacts/${id}`);
    check(stillThere.artifact.id === id, "the owner's page is still there");
    const forged = await fetch(`${origin}/mcp`, {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json", authorization: "Bearer cxa_not-a-token" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
    });
    check(forged.status === 401, "an unknown agent token is refused", forged.status);

    // An approval, from the member's inbox, for their agent.
    const { artifact: harbour } = await api(origin, "/api/artifacts", {
      method: "POST",
      body: JSON.stringify({ title: "Harbour log", files: { "index.html": { content: "<title>Harbour log</title><main><h2>Harbour log</h2></main>", encoding: "utf8" } } }),
    });
    // Ada shares it with Sam from the artifact's Share dialog, Can edit;
    // it reaches Sam's gallery.
    await ada.goto(`${origin}/a/${harbour.id}`);
    await ada.getByRole("button", { name: "Share", exact: true }).click();
    const sd = ada.getByRole("dialog", { name: "Share Harbour log" });
    await sd.getByRole("textbox", { name: "GitHub login" }).fill("Sa");
    await sd.locator(".hints button", { hasText: "Sam" }).click();
    await sd.getByRole("combobox", { name: "Access for the person added" }).selectOption("editor");
    await sd.getByRole("button", { name: "Add", exact: true }).click();
    await sd.getByRole("combobox", { name: "Access for Sam" }).waitFor({ timeout: LIMIT });
    await shot(ada, "share-dialog");
    await sd.getByRole("button", { name: "Done" }).click();
    await laptop.goto(`${origin}/`);
    await laptop.locator("a.card", { hasText: "Harbour log" }).waitFor({ timeout: LIMIT });
    check(true, "Ada shares the Harbour log with Sam from Share, and it reaches Sam's gallery");
    const watched = await tool(origin, token, "watch", { url_or_id: harbour.id });
    check(!watched.error, "Sam's agent watches the owner's page", watched.text.slice(0, 300));
    const agents = (await api(origin, `/api/artifacts/${harbour.id}`)).artifact.participants?.agents ?? [];
    const samAgent = agents.find((a) => a.name === "e2e");
    check(samAgent?.handle && samAgent.person?.name === "Sam", "the page's participants name the token's agent, as Sam's", agents);
    await laptop.goto(`${origin}/inbox`);
    await laptop.locator("main").waitFor({ timeout: LIMIT });
    await ada.goto(`${origin}/a/${harbour.id}`);
    const posted = await ada.evaluate(async (hid) => {
      const form = new FormData();
      form.set("anchor", JSON.stringify({ kind: "element", selector: "main", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
      form.set("body", "Log the spring tide.");
      form.set("version", "1");
      const r = await fetch(`/api/artifacts/${hid}/threads`, { method: "POST", body: form });
      return { status: r.status, body: await r.json().catch(() => null) };
    }, harbour.id);
    const tid = posted.body?.thread?.id;
    check(posted.status === 201 && tid, "the owner comments on the page", posted);
    const firsts = await call(ada, "GET", `/api/artifacts/${harbour.id}/approves-first?threads=${tid}`);
    check(firsts.status === 200 && firsts.body?.approves_first?.length === 1 && firsts.body.approves_first[0] !== null, "the Send menu is told Sam approves first", firsts);
    const sent = await call(ada, "POST", `/api/artifacts/${harbour.id}/threads/${tid}/send`, { to: samAgent.handle });
    check(sent.status === 200, "the owner sends the thread to Sam's agent", sent);
    const withheld = await tool(origin, token, "comments_read", { url_or_id: harbour.id, thread_id: tid });
    check(!withheld.text.includes("Log the spring tide."), "the agent reads the owner's comment withheld before Sam decides", withheld.text.slice(0, 300));
    const card = laptop.locator("article.acard", { hasText: "Log the spring tide." });
    await card.waitFor({ timeout: LIMIT });
    check((await card.locator(".agent").textContent()) === "e2e", "the approval card reaches Sam's open inbox live, naming the agent");
    await shot(laptop, "member-inbox-approval");
    await card.getByRole("button", { name: "Allow this thread, including later replies" }).click();
    await card.locator(".outcome").filter({ hasText: /./ }).waitFor({ timeout: LIMIT });
    const got = await until("the agent reads the comment once Sam allows the thread", async () => {
      const r = await tool(origin, token, "comments_read", { url_or_id: harbour.id, thread_id: tid });
      return r.text.includes("Log the spring tide.") && r;
    });
    check(!got.error, "the agent receives the comment Sam allowed from the inbox");
    const narrowed = await call(laptop, "PUT", `/api/agents/${samAgent.handle}/narrow`, {
      access: [{ type: "clax-comments", actions: ["read"], locations: [harbour.id] }],
    });
    check(narrowed.status === 200, "Sam narrows the agent's session to reading comments on one page", narrowed);
    const refusedRead = await tool(origin, token, "read", { url_or_id: harbour.id });
    check(refusedRead.error && refusedRead.text.includes("not_permitted"), "the narrowed agent session is refused reading the page", refusedRead.text.slice(0, 300));
    const stillComments = await tool(origin, token, "comments_read", { url_or_id: harbour.id, thread_id: tid });
    check(stillComments.text.includes("Log the spring tide."), "the narrowed session still reads the comments its narrowing keeps", stillComments.text.slice(0, 300));

    // The page in its artifact page's frame.
    await ada.goto(`${origin}/a/${id}`);
    const af = await contentFrame(ada, id, 1);
    check((await af.locator("#h").textContent({ timeout: LIMIT })) === "Tide tables", "the page renders in the frame");

    // Each artifact's site, on its own host. With the access check
    // (`/site-signin` on the public host, Tasks 3 and 4) a private site
    // needs a site session; before it, the site routes serve anyone.
    const site = siteOf(harbour.id);
    const siteHost = new URL(site).host;
    // Asked by the signed-in owner's browser, whom a signed-out caller's
    // redirect to /signin would not tell from a missing route.
    const signinProbe = await ada.request.get(`${origin}/site-signin?host=${encodeURIComponent(siteHost)}&next=%2F`, { maxRedirects: 0 });
    const accessChecked = signinProbe.status() !== 404;
    if (!accessChecked) skip("the site handshake, world-open and old keyed URL steps", "/site-signin answers 404: this build serves sites without the access check");
    const signedOut = await raw(`${site}/`, { accept: "text/html" });
    check(signedOut.headers.get("referrer-policy") === "no-referrer", "a site response sends no referrer", Object.fromEntries(signedOut.headers));
    if (accessChecked) {
      const to = signedOut.location ? new URL(signedOut.location, site) : null;
      check(
        signedOut.status === 302 && to?.origin === origin && to.pathname === "/site-signin" && to.searchParams.get("host") === siteHost,
        "a private site signed out is sent to /site-signin on the public origin",
        signedOut,
      );
      const sub = await raw(`${site}/`);
      check(siteRefused(sub) && sub.status === 401, "a private site's subresource signed out is 401", sub.status);
    } else {
      check(signedOut.status === 200 && signedOut.text.includes("Harbour log"), "the site host serves the current version at its root", signedOut.status);
    }
    const tab = await ada.context().newPage();
    pages.push(tab);
    await tab.goto(`${site}/`);
    await tab.locator("h2", { hasText: "Harbour log" }).waitFor({ timeout: LIMIT });
    check(new URL(tab.url()).origin === site, accessChecked ? "the owner's browser opens the private site through the handshake" : "the owner's browser opens the site at its root", tab.url());

    // No path-first route of the Worker is reached on a site host: each is
    // the site's own path there, which it does not have.
    const pathFirst = [
      `/git/a-${harbour.id}.git/info/refs?service=git-upload-pack`,
      "/_diag/bindings",
      "/_dev/repos/tokens",
      "/do/spike/notes",
      "/mcp",
      "/api/artifacts",
      "/_worker/alarm",
      "/_internal/pushed",
    ];
    for (const p of pathFirst) {
      const r = await raw(`${site}${p}`, bearer);
      check(siteRefused(r), `${p} on a site host is the site's path, not the route`, { status: r.status, text: r.text.slice(0, 200) });
    }
    check((await raw(`${origin}/_worker/alarm`, bearer)).status === 200, "the test route still answers on the public host");

    // Two artifacts' sites keep their storage apart in one browser.
    await tab.evaluate(() => localStorage.setItem("x", "harbour"));
    await tab.goto(`${siteOf(id)}/`);
    await tab.locator("#h").waitFor({ timeout: LIMIT });
    check((await tab.evaluate(() => localStorage.getItem("x"))) === null, "one site does not see another's localStorage");
    await tab.evaluate(() => localStorage.setItem("x", "tide"));
    await tab.goto(`${site}/`);
    await tab.locator("h2", { hasText: "Harbour log" }).waitFor({ timeout: LIMIT });
    check((await tab.evaluate(() => localStorage.getItem("x"))) === "harbour", "each site keeps its own localStorage");

    // A db document lives in its artifact's own object, not the instance.
    const { artifact: notes } = await api(origin, "/api/artifacts", {
      method: "POST",
      body: JSON.stringify({ title: "Tide notes", capabilities: { db: {}, assets: {} }, files: { "index.html": { content: "<title>Tide notes</title><main><h2>Tide notes</h2></main>", encoding: "utf8" } } }),
    });
    const put = await api(origin, `/api/artifacts/${notes.id}/docs/tasks/t1`, { method: "PUT", body: JSON.stringify({ data: { title: "Ship" } }) });
    check(put.doc?.data?.title === "Ship", "a db document is written through the API", put);
    const probe = (aid) => api(origin, `/_worker/app-data?artifact=${aid}&path=tasks/t1`);
    const where = await probe(notes.id);
    check(
      where.instance_docs === 0 && where.object_store === true && where.object_doc?.data?.title === "Ship",
      "the document lives in the artifact's object and not in the instance's database",
      where,
    );
    const back = await api(origin, `/api/artifacts/${notes.id}/docs/tasks/t1`);
    check(back.doc?.data?.title === "Ship", "the API reads the document back from the artifact's object", back);
    const other = await probe(harbour.id);
    check(other.object_doc === null, "another artifact's object holds no such document", other);

    // An old keyed asset URL (on the public origin) serves no bytes: at
    // most a redirect to its artifact's site, where the access check is.
    const form = new FormData();
    form.set("file", new Blob(["tide data"], { type: "text/plain" }), "tide.txt");
    const up = await fetch(`${origin}/api/artifacts/${notes.id}/assets`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}` }, body: form });
    const asset = (await up.json()).asset;
    check(up.status === 201 && asset?.id, "an asset is uploaded", asset);
    if (accessChecked) {
      const old = await raw(`${origin}/_blob/${asset.id}?k=${"0".repeat(32)}`);
      check(old.status !== 200 && old.text === "", "an old keyed asset URL serves no bytes", { status: old.status, text: old.text.slice(0, 100) });
      if (old.status >= 300 && old.status < 400) {
        const to = new URL(old.location, origin);
        check(to.origin === siteOf(notes.id) && to.pathname.endsWith(`/${asset.id}`), "an old keyed asset URL redirects to its artifact's site", old.location);
        check(siteRefused(await raw(to.href)), "the redirect's target refuses a caller without a site session");
      }
    }

    // Opening a site to the world (Task 8): served signed out until closed.
    if (accessChecked) {
      const open = await fetch(`${origin}/api/artifacts/${harbour.id}`, { method: "PATCH", headers: { "content-type": "application/json", authorization: `Bearer ${TOKEN}` }, body: JSON.stringify({ site_access: "world" }) });
      const opened = await open.json().catch(() => null);
      if (open.status !== 200 || opened?.artifact?.site_access !== "world") {
        skip("the world-open site steps", `PATCH site_access answers ${open.status}: this build cannot open a site to the world`);
      } else {
        const world = await raw(`${site}/`, { accept: "text/html" });
        check(world.status === 200 && world.text.includes("Harbour log"), "a site opened to the world is served signed out", world.status);
        await api(origin, `/api/artifacts/${harbour.id}`, { method: "PATCH", body: JSON.stringify({ site_access: "private" }) });
        check(!(await raw(`${site}/`, { accept: "text/html" })).text.includes("Harbour log"), "closed again, it refuses the next anonymous request");
      }
    }

    // Deleting the artifact erases its object.
    await api(origin, `/api/artifacts/${notes.id}`, { method: "DELETE" });
    await until("the deleted artifact's object is erased", async () => (await probe(notes.id)).object_store === false);
    check(true, "deleting an artifact erases its object's store");

    // The owner shares the page by link, for comments; a guest names themselves and comments.
    await ada.goto(`${origin}/people`);
    await ada.getByRole("combobox", { name: "Page to share" }).selectOption({ label: "Tide tables" });
    await ada.getByRole("combobox", { name: "What its link allows" }).selectOption("commenter");
    await ada.getByRole("button", { name: "Share by link" }).click();
    await ada.locator(".hosted-links .line", { hasText: "Tide tables" }).waitFor({ timeout: LIMIT });
    const mia = await newPage(browser, "mia");
    pages.push(mia);
    const miaLive = watchLive(mia);
    await mia.goto(`${origin}/a/${id}`);
    const mf = await contentFrame(mia, id, 1);
    check(
      (await postThread(mia, id, "before a name")).code === "guest_name_required",
      "a visitor with the link must give a name to comment",
    );
    await setName(mia, "Mia");
    await comment(mia, "Add the moon phase.");
    check(true, "the guest comments");

    const adaLive = watchLive(ada);
    await ada.goto(`${origin}/a/${id}`);
    const af2 = await contentFrame(ada, id, 1);
    await threads(ada);
    await ada.locator(".thread-card", { hasText: "Add the moon phase." }).waitFor({ timeout: LIMIT });
    check(true, "the owner sees the guest's comment");
    await shot(ada, "owner-view");

    // Live pointers, both ways, over the live sockets.
    // Who each is, as their live socket's welcome names them.
    const welcomed = (seen) =>
      until("a live socket's welcome", () =>
        seen.frames.map((f) => { try { return JSON.parse(f); } catch { return null; } }).filter((j) => j?.t === "welcome").pop()?.person_id,
      );
    const adaId = await welcomed(adaLive);
    const miaId = await welcomed(miaLive);
    const pointerFrom = (seen, person) =>
      seen.frames.some((f) => {
        try {
          const j = JSON.parse(f);
          return j.t === "live" && j.senders.some((s) => s.person_id === person && s.cursor);
        } catch {
          return false;
        }
      });
    const wave = async (page, frame) => {
      const box = await page.locator("iframe.frame").boundingBox();
      const r = await frame.evaluate(() => {
        const b = document.querySelector("#p").getBoundingClientRect();
        return { x: b.x, y: b.y, h: b.height };
      });
      await page.mouse.move(box.x + r.x + 30, box.y + r.y + r.h / 2);
      await page.mouse.move(box.x + r.x + 34, box.y + r.y + r.h / 2);
    };
    const sockets = () => ({ ada: { opened: adaLive.opened, open: adaLive.open, last: adaLive.frames.slice(-2) }, mia: { opened: miaLive.opened, open: miaLive.open, last: miaLive.frames.slice(-2) }, miaId });
    await until("the guest's pointer reaches the owner's live socket", async () => {
      await wave(mia, mf);
      if (pointerFrom(adaLive, miaId)) return true;
      throw new Error(JSON.stringify(sockets()));
    });
    check(true, "the guest's pointer reaches the owner over the live socket");
    await until("the owner's pointer reaches the guest's live socket", async () => {
      await wave(ada, af2);
      return pointerFrom(miaLive, adaId);
    });
    check(true, "the owner's pointer reaches the guest over the live socket");
    await until("the owner's page draws the guest's arrow", () =>
      af2.evaluate(() => (document.querySelector("clax-live")?.shadowRoot?.querySelectorAll(".c").length ?? 0) > 0),
    );
    check(true, "the owner's page draws the guest's pointer, through the bridge in its frame");
    await shot(ada, "live-pointer");

    // Sam, beyond the one seat (the guest holds it, the owner's is reserved).
    const samLive = watchLive(sam);
    await sam.goto(`${origin}/a/${id}`);
    await sam.locator(".banner.spectate").waitFor({ timeout: LIMIT });
    check(true, "Sam is a spectator: the one seat is the guest's");
    await shot(sam, "spectator");
    await until("a spectator's live socket closes", () => samLive.open === 0);
    check(!samLive.frames.some((f) => f.includes("\"welcome\"")), "a spectator holds no live socket: it is closed unwelcomed", samLive);
    const refused = await postThread(sam, id, "a spectator's comment");
    check(refused.status === 403 && refused.code === "spectator", "a spectator's comment is refused", refused);
    const snap = await api(origin, `/api/artifacts/${id}/snapshot`);
    check(snap.seats_free === 0, "the snapshot counts no free seat", snap.seats_free);

    // A simulated wake: the server built anew on the same storage.
    const hib = await fetch(`${origin}/_worker/hibernate`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}` } });
    check(hib.status === 200, "the server is rebuilt on the same storage", hib.status);
    const afterWake = await postThread(sam, id, "straight after the wake");
    check(
      afterWake.status === 403 && afterWake.code === "spectator",
      "after the wake the guest's seat is still held: Sam is still a spectator",
      afterWake,
    );
    const snap2 = await api(origin, `/api/artifacts/${id}/snapshot`);
    check(snap2.seats_free === 0, "after the wake the snapshot still counts no free seat", snap2.seats_free);
    const miaAfter = await postThread(mia, id, "Still here after the wake.");
    check(miaAfter.status === 201, "after the wake the seated guest still comments", miaAfter);
    await ada.locator(".thread-card", { hasText: "Still here after the wake." }).waitFor({ timeout: LIMIT });
    check(true, "after the wake the guest's comment reaches the owner live");
    const presence = await api(origin, `/api/artifacts/${id}/presence`);
    const people = (presence.people ?? presence.presence ?? []).map((p) => p.public_id);
    check(people.includes(miaId) && people.includes(adaId), "after the wake presence still lists the owner and the guest", presence);
    await until("the woken live sockets carry pointers again", async () => {
      await wave(mia, mf);
      const n = adaLive.frames.length;
      await new Promise((r) => setTimeout(r, 300));
      return adaLive.frames.slice(n).some((f) => f.includes(miaId) && f.includes("cursor"));
    });
    check(true, "after the wake the guest's pointer still reaches the owner");

    const health = await fetch(`${origin}/healthz`).then((r) => r.json());
    summary.ok = true;
    summary.version = health.version;
    console.log(JSON.stringify(summary));
  } catch (e) {
    for (const [i, p] of pages.entries()) await p.screenshot({ path: join(dir, `failure-${i}.png`) }).catch(() => {});
    throw e;
  } finally {
    await cleanup();
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
