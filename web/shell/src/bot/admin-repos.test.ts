import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import AdminRepos from "./AdminRepos.svelte";

const PEM = "-----BEGIN RSA PRIVATE KEY-----\nMIIEsecret\n-----END RSA PRIVATE KEY-----\n";
const KEY = { set: true, last4: "a1b2", set_at: "2026-11-05T10:00:00Z", set_by: { id: "*owner", name: "Ana" }, last_ok_at: null, host: "https://api.github.com", problem: null };
const APP = { app_id: 7, slug: "clax-bot", api_url: "https://api.github.com", install_url: "https://github.com/apps/clax-bot/installations/new", key: KEY, set_by: { id: "*owner", name: "Ana" }, set_at: "2026-11-05T10:00:00Z" };
const SAM = { id: "p1", name: "Sam" };
const LINK = { queue_id: "q1", queue_name: "Bugs", repo: "acme/web", repo_url: "https://github.com/acme/web.git", forge_api_url: "https://api.github.com", access: "github_app", installation_id: 42, account: "acme-inc", status: "needs_approval", approved: false, approvable: true, rev: "r1", public: false, allow_open_egress: false, reviewed_previews: false, preview_url: "https://pr-{pr}.acme.dev", preview_ready_path: "/healthz", protected_paths: [], ci_accepted: true, maker: SAM, admins: [SAM, { id: "p2", name: "Mia" }], set_by: SAM, set_at: "2026-11-05T11:00:00Z" };
const view = (o: Record<string, unknown> = {}) => ({ app: APP, links: [], token_approval: false, kek: { set: true }, runtime: { configured: true, third_party: true }, ...o });
const ROUTE = "/api/admin/repos";
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const type = (el: Element | null, v: string) => flush(() => { const i = el as HTMLInputElement; i.value = v; i.dispatchEvent(new Event("input", { bubbles: true })); });
const button = (root: ParentNode, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent!.trim().startsWith(text));

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("Admin › Repositories", () => {
  it("sends the key once, with its line breaks, and shows only its digest after", async () => {
    const calls = stubFetch({ [`GET ${ROUTE}`]: view({ app: null }), [`PUT ${ROUTE}/app`]: view() });
    const m = mount(AdminRepos, {});
    await settle();
    expect(m.root.querySelector("[data-key]")!.textContent).toContain("Not set");
    type(m.root.querySelector("#ar-id"), "7");
    type(m.root.querySelector("#ar-slug"), "clax-bot");
    type(m.root.querySelector("#ar-pem"), PEM);
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toEqual({ app_id: 7, slug: "clax-bot", api_url: "https://api.github.com", private_key: PEM });
    expect(m.root.querySelector<HTMLTextAreaElement>("#ar-pem")!.value).toBe("");
    expect(m.root.querySelector("[data-key]")!.textContent).toContain("Set, digest a1b2");
    expect(m.root.textContent).not.toContain("MIIEsecret");
  });

  it("leaves the key out when none is pasted", async () => {
    const calls = stubFetch({ [`GET ${ROUTE}`]: view(), [`PUT ${ROUTE}/app`]: view() });
    const m = mount(AdminRepos, {});
    await settle();
    type(m.root.querySelector("#ar-api"), "https://api.github.com/");
    type(m.root.querySelector("#ar-slug"), "renamed");
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toEqual({ app_id: 7, slug: "renamed", api_url: "https://api.github.com" });
  });

  it("warns before a move that clears the key and voids approvals, and stops if told to", async () => {
    const asked = vi.fn(() => false);
    vi.stubGlobal("confirm", asked);
    const calls = stubFetch({ [`GET ${ROUTE}`]: view() });
    const m = mount(AdminRepos, {});
    await settle();
    type(m.root.querySelector("#ar-api"), "https://ghe.acme.dev/api/v3");
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(asked).toHaveBeenCalledOnce();
    expect(calls.some(c => c.method === "PUT")).toBe(false);
  });

  it("asks before a blanked forge moves the App to GitHub's", async () => {
    const asked = vi.fn(() => false);
    vi.stubGlobal("confirm", asked);
    const ghe = { ...APP, api_url: "https://ghe.acme.dev/api/v3" };
    const calls = stubFetch({ [`GET ${ROUTE}`]: view({ app: ghe }) });
    const m = mount(AdminRepos, {});
    await settle();
    type(m.root.querySelector("#ar-api"), "");
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(asked).toHaveBeenCalledOnce();
    expect(calls.some(c => c.method === "PUT")).toBe(false);
  });

  it("lists token links too, says what an approval gives, and disconnects any", async () => {
    vi.stubGlobal("confirm", vi.fn(() => true));
    const token = { ...LINK, access: "token", installation_id: null, status: "ready", approvable: false, repo_url: "https://git.acme.dev/a/b.git", forge_api_url: "https://api.acme.dev" };
    let links: unknown[] = [token];
    const calls = stubFetch({ [`GET ${ROUTE}`]: () => view({ links }), [`DELETE ${ROUTE}/links/q1`]: () => { links = []; return {}; } });
    const m = mount(AdminRepos, {});
    await settle();
    const line = m.root.querySelector('[data-link="q1"]')!;
    expect(line.textContent).toContain("made by Sam");
    expect(line.textContent).toContain("admins Sam, Mia");
    expect(line.textContent).toContain("git.acme.dev and api.acme.dev");
    expect(button(line, "Approve")).toBeUndefined();
    expect(m.root.textContent).toContain("lets ClaxBot read that repository, push branches to it and open pull requests");
    expect(m.root.querySelector("[data-runtime]")!.textContent).toContain("third-party runtime");
    click(button(line, "Disconnect"));
    await settle();
    expect(calls.some(c => c.method === "DELETE")).toBe(true);
    expect(m.root.textContent).toContain("No queue has a repository.");
  });

  it("says how many links go live before turning approval off, and stops if told to", async () => {
    const asked = vi.fn(() => false);
    vi.stubGlobal("confirm", asked);
    const waiting = { ...LINK, access: "token", installation_id: null };
    const calls = stubFetch({ [`GET ${ROUTE}`]: view({ token_approval: true, links: [{ ...waiting, queue_id: "q3" }, { ...waiting, queue_id: "q2" }, LINK] }) });
    const m = mount(AdminRepos, {});
    await settle();
    click(m.root.querySelector('[role="switch"]'));
    await settle();
    expect(asked).toHaveBeenCalledWith(expect.stringContaining("2 links go live"));
    expect(calls.some(c => c.method === "PUT")).toBe(false);
  });

  it("turns approval of token links on", async () => {
    const calls = stubFetch({ [`GET ${ROUTE}`]: view(), [`PUT ${ROUTE}/policy`]: view({ token_approval: true }) });
    const m = mount(AdminRepos, {});
    await settle();
    click(m.root.querySelector('[role="switch"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toMatchObject({ url: `${ROUTE}/policy`, body: { token_approval: true } });
  });

  it("approves what the owner reads: that repository, that installation, that queue", async () => {
    const approved = { ...LINK, approved: true, status: "ready" };
    let links = [LINK];
    const calls = stubFetch({
      [`GET ${ROUTE}`]: () => view({ links }),
      [`PUT ${ROUTE}/links/q1`]: () => { links = [approved]; return {}; },
    });
    const m = mount(AdminRepos, {});
    await settle();
    const line = m.root.querySelector('[data-link="q1"]')!;
    expect(line.textContent).toContain("acme/web · installed on acme-inc, installation 42 · waiting for approval");
    click(button(line, "Approve"));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toEqual({ rev: "r1", approved: true });
    expect(button(m.root.querySelector('[data-link="q1"]')!, "Withdraw")).toBeDefined();
  });

  it("shows the owner what a link will do, above its Approve", async () => {
    const line = async (o: Record<string, unknown>) => {
      stubFetch({ [`GET ${ROUTE}`]: view({ links: [{ ...LINK, ...o }] }) });
      const m = mount(AdminRepos, {});
      await settle();
      const text = m.root.querySelector('[data-link="q1"] [data-terms]')!.textContent!;
      document.body.replaceChildren();
      return text;
    };
    const plain = await line({});
    expect(plain).toContain("Private");
    expect(plain).toContain("https://pr-{pr}.acme.dev");
    expect(plain).toContain("/healthz");
    expect(plain).toContain("Protected paths: only the defaults");
    expect(plain).toContain("CI may run ClaxBot's branch, with its secrets and network, before anyone reviews it.");
    expect(plain).not.toContain("Open network");
    expect(await line({ protected_paths: ["infra/", "Makefile"] })).toContain("Protected paths: infra/, Makefile");
    expect(await line({ public: true })).toContain("Marked public: what ClaxBot writes goes straight to reporters.");
    expect(await line({ allow_open_egress: true })).toContain("Open network: anything steering ClaxBot could send the source anywhere.");
    expect(await line({ reviewed_previews: true })).toContain("Previews reach reporters: its admins say a person approves each deploy.");
    expect(await line({ ci_accepted: false })).toContain("CI not accepted yet");
    expect(await line({ public: true, ci_accepted: false })).not.toContain("CI not accepted yet");
    expect(plain).not.toContain("A reporter's reply");
    expect(await line({ wake_on_reply: true })).toContain("A reporter's reply can start a run: it spends credits, and sets ClaxBot working on their words.");
  });

  it("marks a reporter's reply starting a run among the link's dangers", async () => {
    stubFetch({ [`GET ${ROUTE}`]: view({ links: [{ ...LINK, ci_accepted: false, wake_on_reply: true }] }) });
    const m = mount(AdminRepos, {});
    await settle();
    expect([...m.root.querySelectorAll('[data-link="q1"] [data-risk]')].map(e => e.textContent)).toEqual(["A reporter's reply can start a run: it spends credits, and sets ClaxBot working on their words."]);
  });

  it("marks what makes a link dangerous, on a link that needed no approval too", async () => {
    stubFetch({ [`GET ${ROUTE}`]: view({ links: [{ ...LINK, access: "token", installation_id: null, status: "ready", approvable: false, approved: true, allow_open_egress: true }] }) });
    const m = mount(AdminRepos, {});
    await settle();
    expect([...m.root.querySelectorAll('[data-link="q1"] [data-risk]')].map(e => e.textContent)).toEqual([
      "Open network: anything steering ClaxBot could send the source anywhere.",
      "CI may run ClaxBot's branch, with its secrets and network, before anyone reviews it.",
    ]);
  });

  it("names what a link still waits for", async () => {
    const said = async (status: string) => {
      stubFetch({ [`GET ${ROUTE}`]: view({ links: [{ ...LINK, status }] }) });
      const m = mount(AdminRepos, {});
      await settle();
      const text = m.root.querySelector('[data-link="q1"]')!.textContent!;
      document.body.replaceChildren();
      return text;
    };
    expect(await said("needs_read_token")).toContain("no read token yet");
    expect(await said("needs_ci_acceptance")).toContain("waiting for its admins to accept CI");
  });

  it("says an allow-list holding a host people upload to does not stop source reaching it", async () => {
    stubFetch({ [`GET ${ROUTE}`]: view() });
    const m = mount(AdminRepos, {});
    await settle();
    expect(m.root.querySelector("[data-allowlist]")!.textContent).toContain("a host people can upload to, such as github.com");
  });

  it("shows the owner-only refusal an admin who types the URL gets", async () => {
    stubFetch({ [`GET ${ROUTE}`]: [403, { error: { code: "owner_only", message: "only the owner sees and sets the instance's repositories" } }] });
    const m = mount(AdminRepos, {});
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Only the owner sees and sets the instance's repositories.");
  });
});
