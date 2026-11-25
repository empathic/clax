// GitHub sign-in on a local Clax (`[github]` in config.toml), end to end
// against a fake GitHub (e2e/fake-github.ts): the owner adds @octo-cat to a
// queue in the Manage panel and sees the waiting row; octo-cat, in a browser
// of their own, signs in through the fake, finds the queue in their Queues
// list and the notice in their inbox, and the owner's panel lists them as a
// member, no longer waiting.
import { startFakeGitHub } from "./fake-github";
import { NO_KEY_CONFIG, asStranger, expect, startDaemon, test } from "./fixtures";

test("a login added on a local Clax signs in with GitHub and finds the queue and the notice", async ({ browser }) => {
  const gh = await startFakeGitHub("e2e-client", "e2e-secret");
  const config = `${NO_KEY_CONFIG}\n[github]\nclient_id = "e2e-client"\nclient_secret_env = "CLAX_E2E_GITHUB_SECRET"\noauth_base = "${gh.base}"\napi_base = "${gh.base}"\n`;
  const d = await startDaemon({ config, env: { CLAX_E2E_GITHUB_SECRET: "e2e-secret" } });
  try {
    const ownerContext = await browser.newContext();
    const owner = await ownerContext.newPage();
    await owner.goto(`${d.base}/queues`);
    await owner.getByRole("button", { name: "New queue" }).click();
    const form = owner.locator("form.qform");
    await form.getByLabel("Name", { exact: true }).fill("Local bugs");
    await form.getByLabel("Origins, one per line").fill("https://app.example.com");
    await form.getByRole("button", { name: "Create queue" }).click();
    const card = owner.locator("li.qcard", { has: owner.getByRole("heading", { name: "Local bugs" }) });
    await expect(card).toBeVisible();
    await card.getByRole("button", { name: "Manage" }).click();

    // Add @octo-cat: offered because this Clax signs people in with GitHub.
    await card.getByLabel("Add a person").fill("@octo-cat");
    await card.getByRole("button", { name: "Add @octo-cat" }).click();
    const waiting = card.getByRole("list", { name: "Waiting for their GitHub sign-in" });
    await expect(waiting).toContainText("@octo-cat · waiting for their GitHub sign-in");

    // octo-cat signs in, in a browser of their own, through the fake.
    gh.account(7, "octo-cat", "Octo Cat");
    gh.signInAs(7);
    const catContext = await browser.newContext();
    await asStranger(catContext);
    const cat = await catContext.newPage();
    await cat.goto(`${d.base}/auth/github?next=/queues`);
    await expect(cat).toHaveURL(`${d.base}/queues`);
    await expect(cat.getByRole("heading", { name: "Local bugs" })).toBeVisible();
    await cat.goto(`${d.base}/inbox`);
    await expect(cat.getByText(/added you to Local bugs as triage/).first()).toBeVisible();

    // The owner's panel: a member now, no longer waiting.
    await owner.reload();
    const card2 = owner.locator("li.qcard", { has: owner.getByRole("heading", { name: "Local bugs" }) });
    await card2.getByRole("button", { name: "Manage" }).click();
    await expect(card2.getByRole("region", { name: "Members of Local bugs" }).or(card2.locator("section[aria-label='Members of Local bugs']"))).toContainText("Octo Cat");
    await expect(card2.getByRole("list", { name: "Waiting for their GitHub sign-in" })).toHaveCount(0);
    await catContext.close();
    await ownerContext.close();
  } finally {
    await d.stop();
    await gh.stop();
  }
});
