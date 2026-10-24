// People (`/people`, spec 2026-10-08-multi-agent-multi-person §3.1, §3.4,
// §9.6, §17) in a browser: the owner invites a person, who connects with
// `clax connect` from a scratch home and registers an agent; the owner
// sees them appear; the person's browser (made theirs by the browser link)
// sees only themselves and narrows their machine; the owner raises the
// person's agent to publish versions of one artifact (§3.4), from the
// keyboard, and the person sees the raise; a person with a name of the
// daemon's longest (60 characters) connects and the page stays within a
// phone's width; the owner turns the `*unconnected` switch off and on, and
// revokes the person, whose open page then says People is not theirs. The daemon
// listens on every interface, so its invites name this machine's LAN
// address; on a machine with none the daemon refuses invites
// (`not_on_lan`) and the test says so and skips.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { api, expect, publish, startDaemon, test } from "./fixtures";

/** Runs the CLI as the person, on their scratch home, outside any agent channel. */
function asPerson(home: string, args: string[], input?: string, cwd?: string) {
  const env: NodeJS.ProcessEnv = { ...process.env, CLAX_HOME: home };
  for (const k of ["CLAX_CHANNEL", "CLAUDECODE", "CODEX_SESSION_ID", "CLAX_PORT"]) delete env[k];
  return spawnSync(process.env.CLAX_E2E_BIN!, args, { env, input, cwd, encoding: "utf8", timeout: 20_000 });
}

test("the owner invites a person who connects, narrows their machine from their own People, and is revoked", async ({ browser }) => {
  const d = await startDaemon({ bind: "0.0.0.0" });
  const sam = mkdtempSync(join(tmpdir(), "clax-e2e-sam-"));
  const wes = mkdtempSync(join(tmpdir(), "clax-e2e-wes-"));
  const ownerCtx = await browser.newContext();
  const personCtx = await browser.newContext();
  try {
    const owner = await ownerCtx.newPage();
    await owner.goto(`${d.base}/people`);
    await expect(owner.getByRole("heading", { name: "People", level: 2 })).toBeVisible();
    await expect(owner.locator(".gbar a.people-link")).toHaveAttribute("aria-current", "page");
    await owner.getByRole("button", { name: "Invite a person" }).click();
    await expect(owner.getByRole("textbox", { name: "Name", exact: true })).toBeFocused();
    await expect(owner.getByRole("textbox", { name: "Name", exact: true })).toHaveAttribute("maxlength", "60");
    await owner.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
    await owner.getByRole("button", { name: "Create invite" }).click();
    const made = owner.locator(".invite code").or(owner.locator('.invite [role="alert"]'));
    await expect(made).toBeVisible();
    const alert = owner.locator('.invite [role="alert"]');
    const refused = (await alert.count()) ? await alert.textContent() : null;
    test.skip(!!refused && refused.includes("LAN"), `no LAN address here: ${refused}`);
    const command = (await owner.locator(".invite code").textContent())!;
    expect(command).toMatch(/^clax connect http:\/\/\S+\/i\/[0-9A-Z-]+$/);
    await expect(owner.locator(".invite")).toContainText("Use a trusted LAN: Clax traffic is not encrypted");
    await expect(owner.locator(".invite")).toContainText("Expires in 24 h, one use");
    // The first invite carries the switch; the page shows it once.
    await expect(owner.locator('.invite [role="switch"]')).toBeVisible();
    await expect(owner.getByRole("switch")).toHaveCount(1);

    // Sam connects from a scratch home, as `clax connect` on Sam's machine.
    const out = asPerson(sam, ["connect", command.replace(/^clax connect /, ""), "--name", "sam-laptop", "--yes", "--no-browser"]);
    expect(out.status, out.stderr).toBe(0);
    const link = /http:\/\/\S+\/connect\/b\/\S+/.exec(out.stdout + out.stderr)?.[0];
    expect(link, out.stdout + out.stderr).toBeTruthy();
    // The owner's page follows the people topic: Sam and the machine appear.
    const card = owner.locator("article.person[data-person]").filter({ has: owner.locator("h3", { hasText: /^Sam$/ }) });
    await expect(card.locator(".machine .mname")).toHaveText("sam-laptop");
    await expect(card.locator(".machine .key")).toContainText("key SHA256:");

    // One agent of Sam's, registered as Claude Code's SessionStart hook does
    // (a hook's join sends its repository, so it is named after its
    // checkout, sam-notes: spec §11).
    const project = join(sam, "work", "sam-notes");
    mkdirSync(join(project, ".git"), { recursive: true });
    writeFileSync(join(project, ".git", "HEAD"), "ref: refs/heads/main\n");
    const hook = asPerson(sam, ["hook", "--agent", "claude", "session-start"], JSON.stringify({ session_id: "e2e-sam-1", cwd: project, hook_event_name: "SessionStart", source: "startup" }), project);
    expect(hook.status, hook.stderr).toBe(0);
    await expect(card.locator(".agent .aname")).toHaveText("sam-notes");
    await expect(card.locator(".agent")).toContainText("sam-laptop");

    // Sam's browser: the link makes it Sam's, and People shows Sam alone.
    const person = await personCtx.newPage();
    await person.goto(link!);
    await person.waitForURL(/\/people$/);
    await expect(person.locator("article.person")).toHaveCount(1);
    await expect(person.locator("article.person h3")).toContainText("Sam");
    await expect(person.getByRole("button", { name: "Invite a person" })).toHaveCount(0);
    await expect(person.getByRole("switch")).toHaveCount(0);
    await expect(person.locator(".gbar a.people-link")).toBeVisible();
    // Sam turns Delete artifacts off for every agent of the machine.
    await person.locator(".machine summary").click();
    await person.locator(".machine .caps label", { hasText: "Delete artifacts" }).locator("input").uncheck();
    await expect.poll(async () => {
      const g = (await api(d.base, d.token, "/api/people")).people.find((p: { display_name: string }) => p.display_name === "Sam").grants[0];
      return g.narrow ? JSON.stringify(g.narrow).includes('"delete"') : "none";
    }).toBe(false);
    await expect(person.locator(".machine .caps label", { hasText: "Delete artifacts" }).locator("input")).not.toBeChecked();

    // The owner raises Sam's agent, a Collaborator's, to publish versions of
    // one artifact (§3.4), from the keyboard: the focus stays on the checkbox.
    const qr = (await publish(d.base, d.token, "Quarterly Review", { "index.html": "<h1>Quarterly Review</h1>" })).artifact.id;
    await card.locator(".raise summary").click();
    const where = card.locator(".raise .where select");
    await expect(where.locator("option", { hasText: "Quarterly Review" })).toHaveCount(1);
    await where.selectOption({ label: "Quarterly Review" });
    const publishBox = card.locator(".raise .caps label", { hasText: "Publish new versions" }).locator("input");
    await expect(publishBox).toBeEnabled();
    await expect(publishBox).not.toBeChecked();
    await publishBox.focus();
    await owner.keyboard.press("Space");
    await expect.poll(async () => {
      const p = (await api(d.base, d.token, "/api/people")).people.find((x: { display_name: string }) => x.display_name === "Sam");
      return p.agents[0].elevate;
    }).toEqual([{ type: "clax-artifacts", actions: ["update"], locations: [qr] }]);
    await expect(card.locator(".raises")).toHaveText("Publish new versions on “Quarterly Review”");
    await expect(publishBox).toBeChecked();
    await expect(publishBox).toBeFocused();
    // Sam sees the raise beside his own narrowing.
    await person.reload();
    await person.locator(".agent .narrow summary").click();
    await expect(person.locator(".agent .narrow .caps label", { hasText: "Publish new versions" })).toContainText("Raised by the owner on “Quarterly Review” until this session ends");

    // The switch, off then on again: the owner's approval of viewers who are not connected.
    await owner.getByRole("button", { name: "Done" }).click();
    await expect(owner.getByRole("button", { name: "Invite a person" })).toBeFocused();
    const sw = owner.getByRole("switch");
    await expect(sw).toHaveAttribute("aria-checked", "true");
    await sw.click();
    await expect(sw).toHaveAttribute("aria-checked", "false");
    expect((await api(d.base, d.token, "/api/people")).unconnected.on).toBe(false);
    await sw.click();
    await expect(sw).toHaveAttribute("aria-checked", "true");
    expect((await api(d.base, d.token, "/api/people")).unconnected.on).toBe(true);

    // A person with a name of the daemon's longest: at a phone's width the
    // page does not scroll sideways.
    const long = "W".repeat(60);
    await owner.getByRole("button", { name: "Invite a person" }).click();
    await owner.getByRole("textbox", { name: "Name", exact: true }).fill(long);
    await owner.getByRole("button", { name: "Create invite" }).click();
    const longCommand = (await owner.locator(".invite code").textContent())!;
    const joined = asPerson(wes, ["connect", longCommand.replace(/^clax connect /, ""), "--name", "wes-laptop", "--yes", "--no-browser"]);
    expect(joined.status, joined.stderr).toBe(0);
    await owner.getByRole("button", { name: "Done" }).click();
    const longCard = owner.locator("article.person[data-person]").filter({ has: owner.locator("h3", { hasText: long }) });
    await expect(longCard.getByRole("button", { name: `Revoke ${long}` })).toHaveText("Revoke");
    await expect(longCard.getByRole("button", { name: `Always allow ${long}` })).toBeVisible();
    await owner.setViewportSize({ width: 390, height: 844 });
    await expect.poll(() => owner.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(0);
    await owner.setViewportSize({ width: 1280, height: 800 });

    // The owner revokes Sam: Sam is history, a card under Revoked saying
    // when, with no controls (a local daemon reinstates no one: a revoked
    // person takes a new invite), and Sam's open page says People is not his.
    owner.once("dialog", dialog => void dialog.accept());
    await card.getByRole("button", { name: "Revoke Sam" }).click();
    await expect(owner.locator(".revoked")).toContainText("Sam");
    const sams = owner.locator("article.person[data-person]").filter({ has: owner.locator("h3", { hasText: /^Sam$/ }) });
    await expect(sams).toHaveCount(1);
    const history = owner.locator(".revoked article.person[data-person]").filter({ has: owner.locator("h3", { hasText: /^Sam$/ }) });
    await expect(history.locator("header .meta")).toHaveText(/^revoked \d{4}-\d{2}-\d{2}$/);
    await expect(history.getByRole("button")).toHaveCount(0);
    await expect(history.locator("select.preset, .machine, .agent")).toHaveCount(0);
    await expect(owner.getByRole("heading", { name: "People", level: 2 })).toBeFocused();
    await expect(person.locator(".ppl")).toContainText("You are no longer connected: the owner disconnected you.");
    await expect(person.locator("article.person")).toHaveCount(0);
    await expect(person.locator(".gbar a.people-link")).toHaveCount(0);
    await person.reload();
    await expect(person.locator(".ppl")).toContainText("People is for the owner of this Clax and the people they connect.");
    await expect(person.locator(".gbar a.people-link")).toHaveCount(0);
  } finally {
    await ownerCtx.close();
    await personCtx.close();
    await d.stop();
    rmSync(sam, { recursive: true, force: true });
    rmSync(wes, { recursive: true, force: true });
  }
});
