// A thread held for an agent that lost access (spec 2026-10-08-multi-agent-
// multi-person §7.5, §10): Mia's comment sent to Sam's agent waits for
// Sam's approval; the owner then narrows Sam's rights so the agent may no
// longer read the page, and the agent's next take is told so. The owner and
// Sam read the wait with the lost access beside it. The daemon listens on
// every interface, so its invites name this machine's LAN address; on a
// machine with none the test skips.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { api, expect, publish, startDaemon, test } from "./fixtures";
import { type Person, connect, invite, leave, threadFrom } from "./people-helpers";

/** Runs the CLI as the person, on their scratch home, outside any agent channel. */
function asPerson(home: string, args: string[], input?: string, cwd?: string) {
  const env: NodeJS.ProcessEnv = { ...process.env, CLAX_HOME: home };
  for (const k of ["CLAX_CHANNEL", "CLAUDECODE", "CODEX_SESSION_ID", "CLAX_PORT"]) delete env[k];
  return spawnSync(process.env.CLAX_E2E_BIN!, args, { env, input, cwd, encoding: "utf8", timeout: 20_000 });
}

/** A JSON request from `page`'s browser (its credentials, same origin). */
async function from(page: Page, method: string, path: string, body: unknown): Promise<any> {
  return page.evaluate(async ([m, p, b]) => {
    const res = await fetch(p as string, { method: m as string, headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
    if (!res.ok) throw new Error(`${p}: ${res.status} ${await res.text()}`);
    return res.json();
  }, [method, path, body]);
}

test("the owner and the agent's person read a wait beside the lost access", async ({ browser }) => {
  const d = await startDaemon({ bind: "0.0.0.0" });
  const sam = mkdtempSync(join(tmpdir(), "clax-e2e-sam-"));
  const ownerCtx = await browser.newContext();
  const personCtx = await browser.newContext();
  let mia: Person | undefined;
  try {
    const owner = await ownerCtx.newPage();
    await owner.goto(`${d.base}/people`);
    await owner.getByRole("button", { name: "Invite a person" }).click();
    await owner.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
    await owner.getByRole("button", { name: "Create invite" }).click();
    await expect(owner.locator(".invite code").or(owner.locator('.invite [role="alert"]'))).toBeVisible();
    const alert = owner.locator('.invite [role="alert"]');
    const refused = (await alert.count()) ? await alert.textContent() : null;
    test.skip(!!refused && refused.includes("LAN"), `no LAN address here: ${refused}`);
    const command = (await owner.locator(".invite code").textContent())!;
    const out = asPerson(sam, ["connect", command.replace(/^clax connect /, ""), "--name", "sam-laptop", "--yes", "--no-browser"]);
    expect(out.status, out.stderr).toBe(0);
    const link = /http:\/\/\S+\/connect\/b\/\S+/.exec(out.stdout + out.stderr)![0];
    const project = join(sam, "work", "sam-notes");
    mkdirSync(join(project, ".git"), { recursive: true });
    writeFileSync(join(project, ".git", "HEAD"), "ref: refs/heads/main\n");
    const hook = (event: string, name: string) =>
      asPerson(sam, ["hook", "--agent", "claude", event], JSON.stringify({ session_id: "e2e-sam-1", cwd: project, hook_event_name: name, source: "startup", prompt: "Carry on." }), project);
    expect(hook("session-start", "SessionStart").status).toBe(0);

    const aid = (await publish(d.base, d.token, "Plan", { "index.html": "<main><h2>Quarterly goals</h2></main>" })).artifact.id;
    const other = (await publish(d.base, d.token, "Elsewhere", { "index.html": "<main><h2>Other</h2></main>" })).artifact.id;
    // Mia, another connected person, asks Sam's agent by name: Sam has not
    // approved her, so the comment waits for his approval.
    mia = await connect(browser, ((await invite(d, "Mia")) as { command: string }).command, "Mia");
    await threadFrom(mia.page, mia.lan, aid, "@sam-notes please widen the goals");

    // The agent's take tells it of the withheld comment, which then waits
    // for Sam's approval: a row no take told of before the narrowing reads
    // only "held: the agent no longer has access".
    const waiting = (p: Page) => p.locator(".thread-card").filter({ hasText: "widen the goals" }).locator(".waiting");
    expect(hook("prompt", "UserPromptSubmit").status).toBe(0);
    await owner.goto(`${d.base}/a/${aid}`);
    await expect(waiting(owner)).toHaveText("waiting for Sam's approval");

    // The owner narrows Sam's rights to another page; the agent's next
    // take tells it it no longer has access.
    const samId = (await api(d.base, d.token, "/api/people")).people.find((p: { display_name: string }) => p.display_name === "Sam").public_id;
    await from(owner, "PUT", `/api/people/${samId}/rights`, { rights: [
      { type: "clax-artifacts", actions: ["read"], locations: [other] },
      { type: "clax-comments", actions: ["read", "reply", "watch"], locations: ["*"] },
      { type: "clax-ask", actions: ["ask"], locations: ["*"] },
    ] });
    const told = hook("prompt", "UserPromptSubmit");
    expect(told.stdout, told.stderr).toContain("you no longer have access to this page");
    expect(told.stdout).not.toContain("widen the goals");

    await owner.reload();
    await expect(waiting(owner)).toHaveText("waiting for Sam's approval · the agent no longer has access");
    const person = await personCtx.newPage();
    await person.goto(link);
    await person.waitForURL(/\/people$/);
    await person.goto(new URL(`/a/${aid}`, link).href);
    await expect(waiting(person)).toHaveText("waiting for your approval · the agent no longer has access");
  } finally {
    await ownerCtx.close();
    await personCtx.close();
    await leave(mia);
    rmSync(sam, { recursive: true, force: true });
    await d.stop();
  }
});
