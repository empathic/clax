// Hosted mode's sign-in in browsers (crates/clax-server/src/hosted.rs): on a
// daemon whose config.toml has `[hosted]`, the first person sets up with
// the secret; they invite Sam, who joins in a second browser; Sam signs a
// third browser in with a one-use link, and makes an agent token whose
// `claude mcp add` command People shows; the owner shares a page by link,
// and a fourth browser with nothing but the link gives a name and comments
// as a guest. No browser here holds the daemon token: every request is the
// session's (the owner's browser holds its session's shell token). A second
// test checks the owner's surfaces (the site in the bar, pin, inbox), a
// person whose rights read one page seeing only it, frames loaded at their
// content key, and a guest naming themselves on a phone. A third has a
// guest show an agent's draft in the frame, at its draft URL, which no
// content key guards. A fourth has a member's agent token connect over
// /mcp/json as an MCP client, the owner's Send menu name it as the
// member's, who approves first, and the member allow the owner from the
// approval card in their own inbox. A daemon showing the bundled Terms of
// Service and Privacy Policy says so under each sign-in button, serves
// both, links them under the gallery and in the artifact view's more menu,
// and asks a person who has not agreed. With CLAX_E2E_SHOTS set it writes
// screenshots to target/hosted-shots/.
import { createServer } from "node:net";
import { type Browser, type Page } from "@playwright/test";
import { NO_KEY_CONFIG, api, expect, publish, publishAs, registerSession, setName, startDaemon, test } from "./fixtures";

const SECRET = "e2e setup secret";

/** A port nothing listens on now. */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a ? resolve(a.port) : reject(new Error("no port"))));
    });
  });
}

async function hostedDaemon(extra = "") {
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n${extra}`;
  const d = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
  test.skip(d.port !== port, `the daemon took port ${d.port}, not ${port}: its public origin would not match`);
  return { d, origin };
}

async function page(browser: Browser): Promise<Page> {
  return (await browser.newContext()).newPage();
}

/** A thread `p`'s browser posts on `aid`, as the shell posts it: its status and error code. */
async function postThread(p: Page, aid: string, body: string) {
  return p.evaluate(async ([id, text]) => {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > h1", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    form.set("body", text);
    form.set("version", "1");
    const r = await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: form });
    const j = await r.json().catch(() => ({}));
    return { status: r.status, code: j?.error?.code ?? null };
  }, [aid, body]);
}

test("setup, an invite joined in a second browser, a sign-in link for a third, and a guest who names themselves", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const { artifact } = await publish(d.base, d.token, "Launch plan", { "index.html": "<title>Launch plan</title><h1>Launch plan</h1><p>Draft</p>" });

    // Setup: the secret from the link, gone from the address bar at once.
    const owner = await page(browser);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await expect(owner.getByRole("heading", { name: "Set up Clax" })).toBeVisible();
    await expect(owner).toHaveURL(`${origin}/setup`);
    await expect(owner.getByLabel("Setup secret")).toHaveValue(SECRET);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    await expect(owner.locator("a.card")).toContainText("Launch plan");
    // Setup is done: the page is gone.
    expect((await owner.request.get(`${origin}/setup`)).status()).toBe(404);

    // The owner invites Sam; the invite is a link.
    await owner.locator("a.people-link").click();
    await owner.waitForURL(`${origin}/people`);
    await expect(owner.locator(".hosted-me")).toContainText("Signed in as Ada (the owner)");
    await owner.getByRole("button", { name: "Invite a person" }).click();
    await expect(owner.getByRole("radio", { name: "Me, on another machine" })).toHaveCount(0);
    await owner.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
    await owner.getByRole("button", { name: "Create invite" }).click();
    const join = (await owner.locator(".invite code").textContent())!;
    expect(join).toMatch(new RegExp(`^${origin}/join/[0-9A-Z-]+$`));
    await expect(owner.locator(".invite")).not.toContainText("trusted LAN");

    // Sam joins in a second browser, under the invite's name.
    const sam = await page(browser);
    await sam.goto(join);
    await expect(sam.getByText("You have been invited as Sam.", { exact: false })).toBeVisible();
    await sam.getByRole("button", { name: "Join as Sam" }).click();
    await sam.waitForURL(`${origin}/`);
    // An invite gives Sam their own space: the owner's page is not shared with Sam.
    await expect(sam.locator("a.card")).toHaveCount(0);
    // The invite is spent.
    const again = await page(browser);
    await again.goto(join);
    await expect(again.getByText("This invite is not valid.")).toBeVisible();

    // Sam signs a third browser in with a one-use link.
    await sam.goto(`${origin}/people`);
    await sam.getByRole("button", { name: "Sign in another device" }).click();
    const link = (await sam.locator(".hosted-me code").textContent())!;
    expect(link).toMatch(new RegExp(`^${origin}/signin/cxl_`));
    // Opened in a browser signed in as someone else, the link says whose
    // account it is and that it would sign them out; nothing is spent
    // without the click.
    await owner.goto(link);
    await expect(owner.getByText("This link signs this browser in as Sam, once.", { exact: false })).toBeVisible();
    await expect(owner.getByTestId("signed-in-now")).toHaveText("You are signed in here as Ada, the owner. Signing in as Sam signs you out of Ada.");
    await expect(owner.getByRole("button", { name: "Sign in as Sam" })).toBeVisible();
    const unconfirmed = await owner.request.post(`${origin}/api/hosted/signin-links/${link.split("/").pop()}/accept`, { headers: { origin }, data: {} });
    expect(unconfirmed.status()).toBe(409);
    expect((await unconfirmed.json()).error.code).toBe("session_exists");
    const phone = await page(browser);
    await phone.goto(link);
    await expect(phone.getByTestId("signed-in-now")).toHaveCount(0);
    await phone.getByRole("button", { name: "Sign in as Sam" }).click();
    await phone.waitForURL(`${origin}/`);
    await phone.goto(`${origin}/people`);
    await expect(phone.locator(".hosted-me")).toContainText("Signed in as Sam");
    const spent = await page(browser);
    await spent.goto(link);
    await expect(spent.getByText("This sign-in link is not valid.", { exact: false })).toBeVisible();
    await expect(spent.getByRole("button", { name: "Sign in as Sam" })).toHaveCount(0);

    // An agent token, with the command Claude Code takes.
    await phone.getByLabel("Machine").fill("sam-laptop");
    await phone.getByRole("button", { name: "Create token" }).click();
    await expect(phone.locator(".minted code").first()).toContainText(`claude mcp add --transport http clax ${origin}/mcp/json --header "Authorization: Bearer cxa_`);
    await expect(phone.locator(".hosted-tokens .tok")).toContainText("sam-laptop");

    // The owner shares the page by link, for comments.
    await owner.goto(`${origin}/people`);
    await owner.getByRole("combobox", { name: "Page to share" }).selectOption({ label: "Launch plan" });
    await owner.getByRole("combobox", { name: "What its link allows" }).selectOption("commenter");
    await owner.getByRole("button", { name: "Share by link" }).click();
    await expect(owner.locator(".hosted-links .line")).toContainText("Launch plan");

    // A fourth browser with the link alone: it reads, and comments once named.
    const guest = await page(browser);
    await guest.goto(`${origin}/a/${artifact.id}`);
    await expect(guest.locator(".topbar h1")).toHaveText("Launch plan");
    expect(await postThread(guest, artifact.id, "before a name")).toEqual({ status: 401, code: "guest_name_required" });
    await guest.goto(`${origin}/`);
    await guest.waitForURL(`${origin}/signin`);
    await expect(guest.getByText("You are not signed in.")).toBeVisible();
    await guest.goto(`${origin}/a/${artifact.id}`);
    await setName(guest, "Mia");
    await expect.poll(() => postThread(guest, artifact.id, "a guest's comment")).toEqual({ status: 201, code: null });
    // The owner sees it, by the guest's name.
    await owner.goto(`${origin}/a/${artifact.id}`);
    await expect(owner.locator(".thread-card").filter({ hasText: "a guest's comment" })).toContainText("Mia");
    // The guest sees nothing but shared pages.
    await guest.goto(`${origin}/signin`);
    await expect(guest.getByText("You are a guest named Mia.", { exact: false })).toBeVisible();

    // Sam signs the phone out.
    await phone.goto(`${origin}/people`);
    await phone.getByRole("button", { name: "Sign out" }).click();
    await phone.waitForURL(`${origin}/signin`);
    await expect(phone.getByText("You are not signed in.")).toBeVisible();
  } finally {
    await d.stop();
  }
});

/** `p` at `width`, in `scheme`, photographed to `target/hosted-shots/<name>-<width>-<scheme>.png`
 * when CLAX_E2E_SHOTS is set (for looking at, not compared). */
async function shots(p: Page, name: string) {
  if (!process.env.CLAX_E2E_SHOTS) return;
  for (const width of [1280, 390]) {
    for (const scheme of ["light", "dark"] as const) {
      await p.setViewportSize({ width, height: width > 600 ? 820 : 844 });
      await p.emulateMedia({ colorScheme: scheme });
      // The scheme's colours transition in: photographed once they are in.
      await p.evaluate(() => Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {}))));
      await p.screenshot({ path: `../target/hosted-shots/${name}-${width}-${scheme}.png` });
    }
  }
  await p.setViewportSize({ width: 1280, height: 820 });
  await p.emulateMedia({ colorScheme: "light" });
}

test("the owner's browser has the owner's surfaces, a person's rights limit theirs, and content frames at its key", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const plan = (await publish(d.base, d.token, "Plan", { "index.html": "<title>Plan</title><h1>Plan</h1><p>Private</p>" })).artifact;
    const budget = (await publish(d.base, d.token, "Budget", { "index.html": "<title>Budget</title><h1>Budget</h1><p>Shared with Sam</p>" })).artifact;
    const owner = await page(browser);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    // The bar names the site; the owner's pin, delete and inbox are there.
    await expect(owner.locator(".gbar .sub")).toHaveText(`${new URL(origin).host} · seen as Ada`);
    await expect(owner.getByRole("button", { name: "Pin Plan" })).toBeVisible();
    await expect(owner.locator("a.inbox-link")).toBeVisible();
    await shots(owner, "gallery-owner");
    await owner.getByRole("button", { name: "Pin Plan" }).click();
    await expect(owner.getByRole("button", { name: "Unpin Plan" })).toBeVisible();
    // The frame loads the version at its content key.
    await owner.goto(`${origin}/a/${plan.id}`);
    await expect(owner.locator("iframe.frame")).toHaveAttribute("src", new RegExp(`/v/1/k/[0-9a-f]{32}/$`));
    await expect(owner.frameLocator("iframe.frame").locator("h1")).toHaveText("Plan");
    await shots(owner, "artifact-owner");

    // Sam may read Budget alone.
    const made = await owner.evaluate(async rights => {
      const r = await fetch("/api/people/invites", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: "Sam", rights }) });
      return (await r.json()).invite.url as string;
    }, [
      { type: "clax-artifacts", actions: ["read"], locations: [budget.id] },
      { type: "clax-comments", actions: ["read", "write", "reply"], locations: [budget.id] },
    ]);
    const sam = await page(browser);
    await sam.goto(made);
    await sam.getByRole("button", { name: "Join as Sam" }).click();
    await sam.waitForURL(`${origin}/`);
    await expect(sam.locator("a.card")).toHaveCount(1);
    await expect(sam.locator("a.card")).toContainText("Budget");
    await expect(sam.getByRole("button", { name: "Pin Budget" })).toHaveCount(0);
    await sam.goto(`${origin}/a/${budget.id}`);
    await expect(sam.frameLocator("iframe.frame").locator("h1")).toHaveText("Budget");
    const missing = await sam.goto(`${origin}/a/${plan.id}`);
    expect(missing?.status()).toBe(404);
    await sam.goto(`${origin}/people`);
    await expect(sam.locator(".hosted-me")).toContainText("Signed in as Sam");
    await shots(sam, "people-person");

    // A guest on a shared page names themselves in the people menu.
    await owner.goto(`${origin}/people`);
    await expect(owner.locator(".hosted-me")).toContainText("Signed in as Ada");
    await shots(owner, "people-owner");
    await owner.getByRole("combobox", { name: "Page to share" }).selectOption({ label: "Plan" });
    await owner.getByRole("button", { name: "Share by link" }).click();
    await expect(owner.locator(".hosted-links .line")).toContainText("Plan");
    const guest = await page(browser);
    await guest.goto(`${origin}/a/${plan.id}`);
    await expect(guest.frameLocator("iframe.frame").locator("h1")).toHaveText("Plan");
    await guest.getByRole("button", { name: "People and agents" }).click();
    await expect(guest.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name")).toBeVisible();
    await shots(guest, "guest-name");
    // On a phone, the name field too.
    const phone = await (await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true })).newPage();
    await phone.goto(`${origin}/a/${plan.id}`);
    await phone.getByRole("button", { name: "People and agents" }).click();
    const field = phone.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
    await expect(field).toBeVisible();
    await expect.poll(async () => (await field.boundingBox())!.x + (await field.boundingBox())!.width).toBeLessThanOrEqual(390);
    if (process.env.CLAX_E2E_SHOTS) {
      // Each scheme from the page's load, as a phone in it would show it.
      for (const scheme of ["light", "dark"] as const) {
        const p = await (await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, colorScheme: scheme })).newPage();
        await p.goto(`${origin}/a/${plan.id}`);
        await p.getByRole("button", { name: "People and agents" }).click();
        await expect(p.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name")).toBeVisible();
        await p.screenshot({ path: `../target/hosted-shots/guest-name-phone-${scheme}.png` });
        await p.goto(`${origin}/signin`);
        await p.screenshot({ path: `../target/hosted-shots/signin-phone-${scheme}.png` });
      }
    }
    await field.fill("Mia");
    await field.press("Enter");
    await expect.poll(() => postThread(phone, plan.id, "from a phone")).toEqual({ status: 201, code: null });
    // The page's ID alone, outside the shell, reads nothing.
    expect((await guest.request.get(`${origin}/c/${plan.id}/v/1/`)).status()).toBe(404);
  } finally {
    await d.stop();
  }
});

test("a guest on a page shared by link sees an agent drafting and shows the draft at its URL", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const s = await registerSession(d.base, d.token, "claude", "hosted-draft");
    const html = "<!doctype html><title>Plan</title><h1>Plan</h1><p>Ship the beta</p>";
    const { artifact } = await publishAs(d.base, d.token, s.id, "Plan", { "index.html": html });
    const owner = await page(browser);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const shared = await owner.evaluate(async id => {
      const r = await fetch(`/api/hosted/artifacts/${id}/link`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ access: "commenter" }) });
      return r.status;
    }, artifact.id);
    expect(shared).toBe(200);

    const guest = await page(browser);
    await guest.goto(`${origin}/a/${artifact.id}`);
    await expect(guest.frameLocator("iframe.frame").locator("h1")).toHaveText("Plan");
    // Unnamed, no drafts.
    expect((await guest.request.get(`${origin}/api/artifacts/${artifact.id}/drafts`)).status()).toBe(401);
    await setName(guest, "Mia");
    await api(d.base, d.token, `/api/sessions/${s.id}/drafts/${artifact.id}`, { method: "PUT", body: JSON.stringify({ files: { "index.html": { content: html.replace("Ship the beta", "Ship the beta in May"), encoding: "utf8" } } }) });
    const banner = guest.locator(".banner.draft");
    await expect(banner).toContainText("claude is drafting…");
    await banner.getByRole("button", { name: "Show draft" }).click();
    // On the artifact's own host or under /c/: no content key either way.
    const url = new RegExp(`(${artifact.id}\\.localhost:\\d+|/c/${artifact.id})/draft/a_[^/]+/[A-Za-z0-9_-]{22}/`);
    await expect(guest.locator("iframe.frame")).toHaveAttribute("src", url);
    await expect(guest.frameLocator("iframe.frame").locator("[data-clax-draft-changed]")).toHaveText("Ship the beta in May");
  } finally {
    await d.stop();
  }
});

/** An MCP client of agent token `token` on `origin`'s /mcp/json, named `name`: its tool calls, in its session. */
async function tokenAgent(origin: string, token: string, name: string) {
  const headers: Record<string, string> = { "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${token}` };
  const rpc = async (id: number, method: string, params: unknown) => {
    const r = await fetch(`${origin}/mcp/json`, { method: "POST", headers, body: JSON.stringify({ jsonrpc: "2.0", id, method, params }) });
    expect(r.status, await r.clone().text()).toBe(200);
    return r;
  };
  const init = await rpc(1, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name, version: "1" } });
  headers["mcp-session-id"] = init.headers.get("mcp-session-id")!;
  return {
    async call(tool: string, args: unknown): Promise<{ isError?: boolean; text: string }> {
      const r = await (await rpc(2, "tools/call", { name: tool, arguments: args })).json();
      return { isError: r.result?.isError, text: (r.result?.content ?? []).map((c: { text?: string }) => c.text ?? "").join("") };
    },
  };
}

test("a member's token agent is named in the owner's Send menu, and the member allows the owner from their inbox", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const budget = (await publish(d.base, d.token, "Budget", { "index.html": "<title>Budget</title><main><h1>Budget</h1><p>Rows</p></main>" })).artifact;
    const owner = await page(browser);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const made = await owner.evaluate(async () => {
      const r = await fetch("/api/people/invites", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: "Sam", rights: "clax:collaborator" }) });
      return (await r.json()).invite.url as string;
    });
    const sam = await page(browser);
    await sam.goto(made);
    await sam.getByRole("button", { name: "Join as Sam" }).click();
    await sam.waitForURL(`${origin}/`);
    const token = await sam.evaluate(async () => {
      const r = await fetch("/api/hosted/agent-tokens", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ label: "sam-laptop" }) });
      return (await r.json()).token as string;
    });
    const agent = await tokenAgent(origin, token, "summary-writer");
    expect((await agent.call("watch", { url_or_id: budget.id })).isError).toBeFalsy();

    // The owner comments; the Send menu names Sam's agent, and that Sam approves first.
    await owner.goto(`${origin}/a/${budget.id}`);
    const tid = await owner.evaluate(async id => {
      const form = new FormData();
      form.set("anchor", JSON.stringify({ kind: "element", selector: "main > h1", quote: "Budget", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
      form.set("body", "Total the rows, please.");
      form.set("version", "1");
      const r = await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: form });
      return (await r.json()).thread.id as string;
    }, budget.id);
    await owner.goto(`${origin}/a/${budget.id}?thread=${tid}`);
    const card = owner.locator(`.thread-card[data-thread="${tid}"]`);
    await card.getByRole("button", { name: "Choose the agent" }).click();
    const menu = card.getByRole("menu");
    const samItem = menu.getByRole("group", { name: "Sam's agents" }).getByRole("menuitemradio");
    await expect(samItem.locator(".send-name")).toHaveText("summary-writer");
    await expect(samItem.locator(".send-first")).toHaveText("Sam approves first");
    await shots(owner, "send-menu-owner");
    await samItem.click();
    await card.getByRole("button", { name: "Send to summary-writer (Sam's)" }).click();
    await expect(card.locator(".waiting")).toHaveText("waiting for Sam's approval");
    expect((await agent.call("comments_read", { url_or_id: budget.id, thread_id: tid })).text).not.toContain("Total the rows");

    // Sam's inbox holds the approval card; he allows the owner for the thread.
    await sam.goto(`${origin}/inbox`);
    const acard = sam.locator("article.acard", { hasText: "Total the rows, please." });
    await expect(acard.locator(".agent")).toHaveText("summary-writer");
    await shots(sam, "inbox-approval-member");
    await acard.getByRole("button", { name: "Allow this thread, including later replies" }).click();
    await expect(acard.locator(".outcome")).not.toBeEmpty();
    await expect.poll(async () => (await agent.call("comments_read", { url_or_id: budget.id, thread_id: tid })).text).toContain("Total the rows, please.");
  } finally {
    await d.stop();
  }
});

test("another machine's Clax signs in by device code: the page names it, warns when it asked from elsewhere, and approves it", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const owner = await page(browser);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    // The machine asks for a code, with no credential.
    const start = await fetch(`${origin}/api/hosted/device/code`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Clax on alex-laptop" }) });
    expect(start.status).toBe(200);
    const { user_code, verification_uri_complete } = await start.json();
    // Asked from another address than the browser's: the page warns.
    await owner.route("**/api/hosted/device/*", async r => {
      if (r.request().method() !== "GET") return r.continue();
      const res = await r.fetch();
      await r.fulfill({ response: res, json: { ...(await res.json()), same_address: false, peer_ip: "203.0.113.9" } });
    });
    await owner.goto(verification_uri_complete);
    await expect(owner).toHaveURL(`${origin}/device`);
    await expect(owner.getByRole("heading", { name: "Connect a Clax" })).toBeVisible();
    await expect(owner.locator(".signin")).toContainText("Clax on alex-laptop, from 203.0.113.9, asks to publish here as you, Ada.");
    await expect(owner.locator(".signin code")).toHaveText(user_code);
    await expect(owner.getByTestId("other-address")).toContainText("Approve only if you started this sign-in yourself");
    await shots(owner, "device-other-address");
    // From the browser's own address: no warning.
    await owner.unroute("**/api/hosted/device/*");
    await owner.goto(verification_uri_complete);
    await expect(owner.locator(".signin")).toContainText("Clax on alex-laptop, from ");
    await expect(owner.getByTestId("other-address")).toHaveCount(0);
    await owner.getByRole("button", { name: "Approve" }).click();
    await expect(owner.locator(".signin")).toContainText("Clax on alex-laptop is signed in as you.");
    await shots(owner, "device-approved");
    // A refused automatic push, as the owner's inbox lists it.
    const item = { id: "01JA00000000000000000000PU", seq: 9, kind: "push", read: false, created_at: new Date().toISOString(), agent: null, artifact: { id: "7q3k9mzx2b4t", title: "Tide tables", kind: "html", page_url: null }, thread: null, reply: null, version: null, published: null, question: null, work: null, gone: false, url: "/a/7q3k9mzx2b4t", push: { n: 4, origin: "https://clax.example", code: "remote_moved", message: "the public copy is at version 5, past version 4 this Clax pushed last: someone published there since", url: "https://clax.example/a/k3m9q2w8x1ab" } };
    await owner.route(/\/api\/inbox(\?.*)?$/, r => (r.request().url().includes("group=agent") ? r.continue() : r.fulfill({ json: { items: [item], next_cursor: null, unread: 1 } })));
    await owner.goto(`${origin}/inbox`);
    await expect(owner.locator(`[data-item="${item.id}"]`)).toContainText("https://clax.example refused v4 of Tide tables");
    await expect(owner.locator(`[data-item="${item.id}"]`)).toContainText("someone published there since");
    await expect(owner.getByRole("button", { name: "Pushes" })).toBeVisible();
    await shots(owner, "inbox-push");
  } finally {
    await d.stop();
  }
});

test("People's hosted part shows a refusal as a sentence", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const owner = await page(browser);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    await owner.goto(`${origin}/people`);
    // The daemon refuses a token in its own words, lower case and with no
    // full stop (here, its rate limit); People says it as a sentence.
    await owner.route("**/api/hosted/agent-tokens", r => (r.request().method() === "POST"
      ? r.fulfill({ status: 429, contentType: "application/json", body: JSON.stringify({ error: { code: "rate_limited", message: "too many tokens made from this address" } }) })
      : r.continue()));
    await owner.getByLabel("Machine").fill("laptop");
    await owner.getByRole("button", { name: "Create token" }).click();
    const alert = owner.getByRole("alert");
    await expect(alert).toHaveText("Too many tokens made from this address.");
    await shots(owner, "hosted-refusal");
  } finally {
    await d.stop();
  }
});

test("the bundled Terms of Service and Privacy Policy: notices, pages, links and the prompt", async ({ browser }) => {
  const { d, origin } = await hostedDaemon('legal = "bundled"\n');
  try {
    const owner = await page(browser);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await expect(owner.getByTestId("legal-notice")).toHaveText("By setting up this Clax, you agree to the Terms of Service and the Privacy Policy.");
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const links = owner.getByRole("navigation", { name: "About Clax" });
    await expect(links.getByRole("link", { name: "Terms of Service" })).toHaveAttribute("href", "/terms");
    await expect(owner.getByTestId("legal-prompt"), "setup agreed").toHaveCount(0);
    await shots(owner, "legal-gallery");

    // The documents are public: a browser that never signed in reads both.
    const anon = await page(browser);
    await anon.goto(`${origin}/terms`);
    await expect(anon.getByRole("heading", { level: 1 })).toHaveText("Terms of Service");
    await anon.getByRole("link", { name: "Privacy Policy" }).first().click();
    await expect(anon.getByRole("heading", { level: 1 })).toHaveText("Privacy Policy");
    await anon.goto(`${origin}/signin`);
    await expect(anon.getByText("Terms of Service · Privacy Policy · Source on GitHub")).toBeVisible();

    // The artifact view's more menu links both.
    const aid = (await publish(d.base, d.token, "Plan", { "index.html": "<title>Plan</title><h1>Plan</h1>" })).artifact.id;
    await owner.goto(`${origin}/a/${aid}`);
    await owner.getByRole("button", { name: "More" }).click();
    await expect(owner.getByRole("menuitem", { name: "Privacy Policy" })).toHaveAttribute("href", "/privacy");
    await owner.keyboard.press("Escape");

    // A person who has not agreed to the current version (here, as the
    // daemon would answer after a material change) is asked once, and
    // Continue records it with the daemon.
    await owner.route("**/api/hosted/me", async r => {
      const res = await r.fetch();
      const body = await res.json();
      await r.fulfill({ response: res, json: { ...body, legal: { ...body.legal, agreed: false } } });
    });
    await owner.goto(`${origin}/`);
    const prompt = owner.getByTestId("legal-prompt");
    await expect(prompt.getByRole("dialog")).toBeVisible();
    await shots(owner, "legal-prompt");
    const agreed = owner.waitForResponse(r => r.url().endsWith("/api/hosted/legal/agree"));
    await prompt.getByRole("button", { name: "Continue" }).click();
    expect((await agreed).status()).toBe(204);
    await expect(prompt).toHaveCount(0);
  } finally {
    await d.stop();
  }
});
