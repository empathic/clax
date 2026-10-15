// Two people end to end (spec 2026-10-08-multi-agent-multi-person §3,
// §7, §9, §10, §11, §17, §18), each in a browser of their own at the
// daemon's LAN address, with their agents as real `clax mcp` shims on their
// scratch homes, each registering its own session and credential:
//
// - the owner invites Sam from People; Mia's comment sent to Sam's agent
//   names who approves first to Mia, its author, and not on the owner's
//   thread she did not write ("Only about their content"); it waits for
//   Sam's approval, which the owner and Mia read and a LAN visitor does not
//   ("sent to sam-notes"), and the agent reads only that something is
//   withheld; Sam allows it from the card above the thread, at desktop and
//   phone width, and his agent reads it;
// - Mia declines Sam's comment in her inbox: Sam and the owner read
//   "declined by Mia", Mia "you declined", a visitor "sent"; Mia's agent
//   replies to the owner's thread and is named as Mia's agent to the owner
//   and as her own to Mia; each agent reads the delivery state of the
//   threads sent to it alone ("Own threads only");
// - Sam raises one of Mia's two agents on his artifact (each agent its own
//   credential: the other is not raised); Mia's narrowing caps the raise
//   and lifting it restores it; Mia removes Sam's raise from her People;
// - Sam's agent loses read access while a comment waits for it: the owner
//   and Sam read "held: the agent no longer has access", the agent is told
//   it no longer has access, and the comment reaches it once Sam gives the
//   access back.
//
// The daemon listens on every interface; on a machine with no LAN address
// it refuses invites (`not_on_lan`) and each test says so and skips.
import type { Page } from "@playwright/test";
import { expect, publish, startDaemon, test } from "./fixtures";
import { Agent, type Person, connect, handleOf, invite, leave, ownerThread, people, project, sendFrom, threadFrom, threadPage } from "./people-helpers";

const PAGE = { "index.html": "<main><h1>Quarterly Review</h1><h2>Quarterly goals</h2></main>" };

test("Mia's comment to Sam's agent waits for Sam's approval, which only those concerned read, and reaches the agent once Sam allows it", async ({ browser }) => {
  const d = await startDaemon({ bind: "0.0.0.0" });
  const ownerCtx = await browser.newContext();
  const visitorCtx = await browser.newContext();
  let sam: Person | undefined;
  let mia: Person | undefined;
  let agent: Agent | undefined;
  try {
    // The owner invites Sam from People.
    const owner = await ownerCtx.newPage();
    await owner.goto(`${d.base}/people`);
    await owner.getByRole("button", { name: "Invite a person" }).click();
    await owner.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
    await owner.getByRole("button", { name: "Create invite" }).click();
    await expect(owner.locator(".invite code").or(owner.locator('.invite [role="alert"]'))).toBeVisible();
    const refused = owner.locator('.invite [role="alert"]');
    test.skip((await refused.count()) > 0 && (await refused.textContent())!.includes("LAN"), "no LAN address here");
    sam = await connect(browser, (await owner.locator(".invite code").textContent())!, "Sam");
    await owner.getByRole("button", { name: "Done" }).click();

    // Sam's agent, registered by its own shim, watches the owner's page;
    // the owner's People lists it under Sam.
    const aid = (await publish(d.base, d.token, "Quarterly Review", PAGE)).artifact.id;
    agent = await Agent.start(sam.home, project(sam.home, "sam-notes"));
    expect((await agent.call("watch", { url_or_id: aid })).watching).toBe(true);
    const card = owner.locator("article.person[data-person]").filter({ has: owner.locator("h3", { hasText: /^Sam$/ }) });
    await expect(card.locator(".agent .aname")).toHaveText("sam-notes");

    // Mia comments on the owner's page from her browser. Her Send menu says
    // Sam approves first for her own comment, and says nothing of it for
    // the owner's thread, which she did not write.
    const m = await invite(d, "Mia");
    test.skip("skip" in m, "skip" in m ? m.skip : "");
    mia = await connect(browser, (m as { command: string }).command, "Mia");
    const tid = await threadFrom(mia.page, mia.lan, aid, "The goals need a Q4 column.");
    const ownersTid = await ownerThread(d, aid, "The owner's own note.");
    await mia.page.goto(threadPage(mia.lan, aid, ownersTid));
    const ownersCard = mia.page.locator(`.thread-card[data-thread="${ownersTid}"]`);
    await ownersCard.getByRole("button", { name: "Choose the agent" }).click();
    const ownersItem = ownersCard.getByRole("menu").getByRole("group", { name: "Sam's agents" }).getByRole("menuitemradio");
    await expect(ownersItem.locator(".send-name")).toHaveText("sam-notes");
    await expect(ownersItem.locator(".send-first")).toHaveCount(0);
    await mia.page.keyboard.press("Escape");
    await mia.page.goto(threadPage(mia.lan, aid, tid));
    const miasCard = mia.page.locator(`.thread-card[data-thread="${tid}"]`);
    await miasCard.getByRole("button", { name: "Choose the agent" }).click();
    const item = miasCard.getByRole("menu").getByRole("group", { name: "Sam's agents" }).getByRole("menuitemradio");
    await expect(item.locator(".send-first")).toHaveText("Sam approves first");
    await item.click();
    await miasCard.getByRole("button", { name: "Send to sam-notes (Sam's)" }).click();
    await expect(miasCard.locator(".waiting")).toHaveText("waiting for Sam's approval");

    // The owner reads whose approval it waits for; a visitor reads only
    // that it was sent; the agent reads that a comment is withheld, never
    // its text nor whose approval.
    await owner.goto(threadPage(d.base, aid, tid));
    await expect(owner.locator(`.thread-card[data-thread="${tid}"] .waiting`)).toHaveText("waiting for Sam's approval");
    const visitor = await visitorCtx.newPage();
    await visitor.goto(threadPage(mia.lan, aid, tid));
    await expect(visitor.locator(`.thread-card[data-thread="${tid}"] .waiting`)).toHaveText("sent to sam-notes");
    await expect.poll(() => agent!.waitBriefly()).toContain("is withheld until your person approves it");
    expect(agent.seen).not.toContain("Q4 column");
    expect(agent.seen).not.toContain("Sam's approval");

    // Sam's browser: the approval card above the thread, at desktop and
    // phone width; he allows the thread there.
    await sam.page.goto(threadPage(sam.lan, aid, tid));
    const above = sam.page.locator(".thread-approval .acard");
    await expect(above.locator(".ahead")).toHaveText(/^Mia wants to reach your agent sam-notes/);
    await expect(above.locator(".held")).toContainText("The goals need a Q4 column.");
    await expect(sam.page.locator(`.thread-card[data-thread="${tid}"] .waiting`)).toHaveText("waiting for your approval");
    await sam.page.screenshot({ path: test.info().outputPath("sam-approval-desktop.png") });
    await sam.page.setViewportSize({ width: 390, height: 844 });
    await expect.poll(() => sam!.page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(0);
    await sam.page.screenshot({ path: test.info().outputPath("sam-approval-phone.png") });
    await sam.page.setViewportSize({ width: 1280, height: 800 });
    await above.getByRole("button", { name: "Allow this thread, including later replies" }).click();
    await expect(above.locator(".outcome")).toHaveText("Allowed in this thread");

    // Sam's agent reads Mia's comment; her thread no longer waits.
    await expect.poll(() => agent!.waitBriefly()).toContain("The goals need a Q4 column.");
    await expect(miasCard).not.toContainText("waiting for Sam's approval");
  } finally {
    agent?.stop();
    await ownerCtx.close();
    await visitorCtx.close();
    await leave(sam);
    await leave(mia);
    await d.stop();
  }
});

test("Mia declines Sam's comment: those concerned read it declined by Mia, a visitor reads it sent; Mia's agent replies under its name and reads only its own threads' states", async ({ browser }) => {
  const d = await startDaemon({ bind: "0.0.0.0" });
  const ownerCtx = await browser.newContext();
  const visitorCtx = await browser.newContext();
  let sam: Person | undefined;
  let mia: Person | undefined;
  const agents: Agent[] = [];
  try {
    const s = await invite(d, "Sam");
    test.skip("skip" in s, "skip" in s ? s.skip : "");
    sam = await connect(browser, (s as { command: string }).command, "Sam");
    mia = await connect(browser, ((await invite(d, "Mia")) as { command: string }).command, "Mia");
    const aid = (await publish(d.base, d.token, "Quarterly Review", PAGE)).artifact.id;
    const miaAgent = await Agent.start(mia.home, project(mia.home, "mia-notes"));
    agents.push(miaAgent);
    const samAgent = await Agent.start(sam.home, project(sam.home, "sam-notes"));
    agents.push(samAgent);
    for (const a of agents) expect((await a.call("watch", { url_or_id: aid })).watching).toBe(true);
    const miaHandle = await handleOf(d, "Mia", "mia-notes");

    // Sam sends his comment to Mia's agent; Mia declines it in her inbox.
    const tid = await threadFrom(sam.page, sam.lan, aid, "Sam asks for a bigger chart.");
    await sendFrom(sam.page, sam.lan, aid, tid, miaHandle);
    await mia.page.goto(`${mia.lan}/inbox`);
    const acard = mia.page.locator(".acard").filter({ hasText: "Sam asks for a bigger chart." });
    await expect(acard.locator(".ahead")).toHaveText(/^Sam wants to reach your agent mia-notes/);
    await acard.getByRole("button", { name: "Decline" }).click();
    await expect(acard.locator(".outcome")).toHaveText("Declined");
    const waiting = (p: Page, origin: string) => p.goto(threadPage(origin, aid, tid)).then(() => p.locator(`.thread-card[data-thread="${tid}"] .waiting`));
    await expect(await waiting(sam.page, sam.lan)).toHaveText("declined by Mia");
    await expect(await waiting(mia.page, mia.lan)).toHaveText("you declined");
    const owner = await ownerCtx.newPage();
    await expect(await waiting(owner, d.base)).toHaveText("declined by Mia");
    await expect(await waiting(await visitorCtx.newPage(), mia.lan)).toHaveText("sent to mia-notes");
    expect(miaAgent.seen).not.toContain("bigger chart");

    // The owner's comment reaches Mia's agent (Mia said yes to the owner's
    // content), which replies: the owner reads the reply as Mia's agent's,
    // Mia as her own.
    const ownersTid = await ownerThread(d, aid, "Please add a legend.");
    await sendFrom(owner, d.base, aid, ownersTid, miaHandle);
    await expect.poll(() => miaAgent.waitBriefly()).toContain("Please add a legend.");
    await miaAgent.call("comments_reply", { url_or_id: aid, thread_id: ownersTid, text: "Added a legend." });
    await owner.goto(threadPage(d.base, aid, ownersTid));
    const reply = (p: Page) => p.locator(`.thread-card[data-thread="${ownersTid}"] .msg.agent .author`);
    await expect(reply(owner)).toHaveText(/^mia-notes \(Mia's agent, via \S.*\)$/);
    await mia.page.goto(threadPage(mia.lan, aid, ownersTid));
    await expect(reply(mia.page)).toHaveText(/^mia-notes \(agent, via \S.*\)$/);

    // Each agent reads the delivery state of the threads sent to it alone.
    const stateOf = async (a: Agent, t: string) => {
      const r = await a.call("comments_read", { url_or_id: aid, thread_id: t });
      const th = (r.threads as { thread_id: string; feedback_state: unknown }[]).find(x => x.thread_id === t);
      return th?.feedback_state ?? null;
    };
    expect(await stateOf(miaAgent, ownersTid)).not.toBeNull();
    expect(await stateOf(samAgent, ownersTid)).toBeNull();
  } finally {
    for (const a of agents) a.stop();
    await ownerCtx.close();
    await visitorCtx.close();
    await leave(sam);
    await leave(mia);
    await d.stop();
  }
});

test("Sam raises one of Mia's agents on his artifact; Mia's narrowing caps the raise, and Mia removes it", async ({ browser }) => {
  const d = await startDaemon({ bind: "0.0.0.0" });
  let sam: Person | undefined;
  let mia: Person | undefined;
  const agents: Agent[] = [];
  try {
    const s = await invite(d, "Sam");
    test.skip("skip" in s, "skip" in s ? s.skip : "");
    sam = await connect(browser, (s as { command: string }).command, "Sam");
    mia = await connect(browser, ((await invite(d, "Mia")) as { command: string }).command, "Mia");
    const samAgent = await Agent.start(sam.home, project(sam.home, "sam-notes"));
    agents.push(samAgent);
    const notes = String((await samAgent.call("publish", { title: "Sam's Notes", html: "<main><h1>Sam's Notes</h1></main>" })).artifact_id);
    // Two agents of Mia's machine, each with its own session and credential.
    const fix = await Agent.start(mia.home, project(mia.home, "notes-fix"));
    const docs = await Agent.start(mia.home, project(mia.home, "notes-docs"));
    agents.push(fix, docs);
    for (const a of [fix, docs]) expect((await a.call("watch", { url_or_id: notes })).watching).toBe(true);
    const handles = (await people(d)).Mia.agents.map(a => a.handle);
    expect(new Set(handles).size).toBe(2);
    const fixHandle = await handleOf(d, "Mia", "notes-fix");
    const publishes = (a: Agent) => a.attempt("publish", { id: notes, html: `<main>${Math.random()}</main>` }).then(r => String(r.error ?? "ok"));
    expect(await publishes(fix)).toContain("not_permitted");

    // Sam, the artifact's creator, raises notes-fix there from his browser.
    const raised = await sam.page.request.put(`${sam.lan}/api/agents/${fixHandle}/elevate`, {
      headers: { origin: sam.lan },
      data: { access: [{ type: "clax-artifacts", actions: ["update"], locations: [notes] }] },
    });
    expect(raised.status(), await raised.text()).toBe(200);
    await expect.poll(() => publishes(fix)).toBe("ok");
    expect(await publishes(docs)).toContain("not_permitted");

    // Mia's People shows Sam's raise; her narrowing of notes-fix caps it,
    // and lifting the narrowing gives it back.
    await mia.page.goto(`${mia.lan}/people`);
    const agentRow = mia.page.locator(".agent").filter({ has: mia.page.locator(".aname", { hasText: /^notes-fix$/ }) });
    await expect(agentRow.locator(".raised-by .note.by")).toHaveText("Raised by Sam until this session ends: Publish new versions on “Sam's Notes”");
    await agentRow.locator(".narrow summary").click();
    const box = agentRow.locator(".narrow .caps label", { hasText: "Publish new versions" }).locator("input");
    await box.uncheck();
    await expect.poll(() => publishes(fix)).toContain("not_permitted");
    await box.check();
    await expect.poll(() => publishes(fix)).toBe("ok");

    // Mia removes Sam's raise: notes-fix may no longer publish there.
    await agentRow.getByRole("button", { name: "Remove Sam's raise of notes-fix" }).click();
    await expect(agentRow.locator(".raised-by")).toHaveCount(0);
    await expect.poll(() => publishes(fix)).toContain("not_permitted");
  } finally {
    for (const a of agents) a.stop();
    await leave(sam);
    await leave(mia);
    await d.stop();
  }
});

test("a comment waiting for Sam's agent is held while the agent has no access, and reaches it once Sam gives the access back", async ({ browser }) => {
  const d = await startDaemon({ bind: "0.0.0.0" });
  const ownerCtx = await browser.newContext();
  let sam: Person | undefined;
  let agent: Agent | undefined;
  try {
    const s = await invite(d, "Sam");
    test.skip("skip" in s, "skip" in s ? s.skip : "");
    sam = await connect(browser, (s as { command: string }).command, "Sam");
    const aid = (await publish(d.base, d.token, "Quarterly Review", PAGE)).artifact.id;
    agent = await Agent.start(sam.home, project(sam.home, "sam-notes"));
    expect((await agent.call("watch", { url_or_id: aid })).watching).toBe(true);
    const handle = await handleOf(d, "Sam", "sam-notes");

    // The owner sends the agent a comment; before the agent reads it, Sam
    // takes Read artifacts from it.
    const owner = await ownerCtx.newPage();
    const tid = await ownerThread(d, aid, "Please check the totals.");
    await sendFrom(owner, d.base, aid, tid, handle);
    await sam.page.goto(`${sam.lan}/people`);
    const row = sam.page.locator(".agent").filter({ has: sam.page.locator(".aname", { hasText: /^sam-notes$/ }) });
    await row.locator(".narrow summary").click();
    const read = row.locator(".narrow .caps label", { hasText: "Read artifacts" }).locator("input");
    await read.uncheck();
    await expect(read).not.toBeChecked();

    // The agent is told it no longer has access; the owner and Sam read the hold.
    await expect.poll(() => agent!.waitBriefly()).toContain("held: you no longer have access to this page");
    expect(agent.seen).not.toContain("Please check the totals.");
    await owner.goto(threadPage(d.base, aid, tid));
    await expect(owner.locator(`.thread-card[data-thread="${tid}"] .waiting`)).toHaveText("held: the agent no longer has access");
    const samsView = await sam.ctx.newPage();
    await samsView.goto(threadPage(sam.lan, aid, tid));
    await expect(samsView.locator(`.thread-card[data-thread="${tid}"] .waiting`)).toHaveText("held: the agent no longer has access");

    // Sam gives the access back: the comment reaches the agent.
    await read.check();
    await expect.poll(() => agent!.waitBriefly()).toContain("Please check the totals.");
    await expect(owner.locator(`.thread-card[data-thread="${tid}"]`)).not.toContainText("no longer has access");
  } finally {
    agent?.stop();
    await ownerCtx.close();
    await leave(sam);
    await d.stop();
  }
});
