import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import RepoPanel from "./RepoPanel.svelte";

const QUEUE = { id: "q1", name: "Bugs", origins: [], reporters_see: "own" as const, public_notice: false, created_at: "2026-10-27T10:00:00Z", role: "admin" as const };
const UNSET = { set: false, last4: null, set_at: null, set_by: null, last_ok_at: null, host: null, problem: null };
const SET = { ...UNSET, set: true, last4: "rite", set_at: "2026-11-05T10:00:00Z", set_by: { id: "*owner", name: "Ana" } };
const NO_APP = { configured: false, install_url: null, forge_api_url: null };
const APP = { configured: true, install_url: "https://github.com/apps/clax-bot/installations/new", forge_api_url: "https://api.github.com" };
const OWN = { configured: true, third_party: false, egress_held: true, failed: false, operator_target: false };
const LINK = {
  rev: "r1", last_ok_at: null, last_failure: null, last_failure_at: null, repo: "acme/web", base: "main", repo_url: "https://github.com/acme/web.git", forge_api_url: "https://api.github.com", protected_paths: [],
  preview_url: "https://pr-{pr}.acme.dev", preview_ready_path: "/healthz", preview_names_commit: true, preview_timeout_secs: 1200, preview_poll_secs: 10,
  access: { kind: "token", read_token: UNSET, write_token: UNSET }, public: false, allow_open_egress: false, reviewed_previews: false, wake_on_reply: false, ci_accepted: false, status: "needs_token", set_by: { id: "*owner", name: "Ana" }, set_at: "2026-11-05T10:00:00Z",
};
const view = (o: Record<string, unknown> = {}) => ({ repo: null, app: NO_APP, runtime: OWN, ...o });
const ROUTE = "/api/queues/q1/repo";
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const type = (el: Element | null, v: string) => flush(() => { const i = el as HTMLInputElement; i.value = v; i.dispatchEvent(new Event("input", { bubbles: true })); });
const button = (root: ParentNode, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent!.trim().startsWith(text));
const open = async (routes: Record<string, unknown>) => {
  const calls = stubFetch(routes);
  const m = mount(RepoPanel, { queue: QUEUE });
  await settle();
  return { calls, m };
};

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("RepoPanel", () => {
  it("connects with tokens by default, sending only what was filled in", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: view(), [`PUT ${ROUTE}`]: view({ repo: LINK }) });
    expect(m.root.textContent).toContain("Not connected.");
    expect(m.root.querySelector('input[name="repo-access"][value="github_app"]')).toBeNull();
    type(m.root.querySelector('input[name="repo"]'), " acme/web ");
    type(m.root.querySelector('input[name="preview_url"]'), "https://pr-{pr}.acme.dev");
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({
      url: ROUTE, method: "PUT",
      body: { repo: "acme/web", base: "main", preview_url: "https://pr-{pr}.acme.dev", protected_paths: [], preview_ready_path: "/healthz", preview_names_commit: true, preview_timeout_secs: 1200, preview_poll_secs: 10, access: { kind: "token" }, public: false, allow_open_egress: false, reviewed_previews: false, wake_on_reply: false, ci_accepted: false, if_rev: null },
    });
    expect(m.root.querySelector(".stat")!.textContent).toContain("Not ready: add a write token and a read token below.");
  });

  it("keeps tokens out of the form: each is set and cleared on its own, and only metadata shows", async () => {
    const taken = { ...LINK, status: "needs_read_token", access: { kind: "token", read_token: UNSET, write_token: SET } };
    const { calls, m } = await open({
      [`GET ${ROUTE}`]: view({ repo: LINK }),
      [`PUT ${ROUTE}/tokens/write`]: view({ repo: taken }),
    });
    const write = m.root.querySelector('[data-token="write"]')!;
    click(button(write, "Set"));
    await settle();
    type(write.querySelector('input[type="password"]'), "github_pat_secret_value");
    click(write.querySelector('button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({ url: `${ROUTE}/tokens/write`, method: "PUT", body: { token: "github_pat_secret_value", if_rev: "r1" } });
    expect(m.root.textContent).not.toContain("github_pat_secret_value");
    expect(m.root.querySelector('[data-token="write"]')!.textContent).toContain("Set, ending rite");
    // A private repository still lacks what a container needs.
    expect(m.root.querySelector(".stat")!.textContent).toContain("Not ready: add a read token below.");
    expect(m.root.textContent).not.toContain("nothing published yet");
    expect(m.root.textContent).toContain("Without a read token only a public repository can be cloned.");
  });

  it("says what each token may do, at the least scope", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }) });
    const text = m.root.textContent!;
    expect(text).toContain("Contents: Read");
    expect(text).toContain("Pull requests: Read and write");
    expect(text).toContain("It stays on this daemon and never reaches a container.");
    expect(text).toContain("Changing the repository or either URL clears both tokens.");
  });

  it("shows a refusal as the daemon words it, the same value in both tokens among them", async () => {
    const { m } = await open({
      [`GET ${ROUTE}`]: view({ repo: LINK }),
      [`PUT ${ROUTE}/tokens/read`]: [400, { error: { code: "same_token", message: "the read and write tokens must differ" } }],
    });
    const read = m.root.querySelector('[data-token="read"]')!;
    click(button(read, "Set"));
    await settle();
    type(read.querySelector('input[type="password"]'), "github_pat_same_value");
    click(read.querySelector('button[type="submit"]'));
    await settle();
    expect(read.querySelector('[role="alert"]')!.textContent).toBe("The read and write tokens must differ.");
  });

  it("offers the App with its install link when the instance has one, and hides the URLs it decides", async () => {
    const appLink = { ...LINK, access: { kind: "github_app", installation_id: 42, account: "acme-inc", approved: false, approved_by: null, approved_at: null }, status: "needs_approval" };
    const { calls, m } = await open({ [`GET ${ROUTE}`]: view({ app: APP }), [`PUT ${ROUTE}`]: view({ app: APP, repo: appLink }) });
    click(m.root.querySelector('input[name="repo-access"][value="github_app"]'));
    const link = m.root.querySelector<HTMLAnchorElement>('a[data-install]')!;
    expect(link.href).toBe(APP.install_url);
    expect(link.rel).toContain("noopener");
    expect(m.root.querySelector('input[name="repo_url"]')).toBeNull();
    expect(m.root.querySelector('input[name="forge_api_url"]')).toBeNull();
    type(m.root.querySelector('input[name="repo"]'), "acme/web");
    type(m.root.querySelector('input[name="preview_url"]'), "https://pr-{pr}.acme.dev");
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    const body = calls.find(c => c.method === "PUT")!.body as Record<string, unknown>;
    expect(body.access).toEqual({ kind: "github_app" });
    expect(m.root.querySelector("[data-account]")!.textContent).toContain("Installed on acme-inc.");
    expect(body).not.toHaveProperty("repo_url");
    expect(body).not.toHaveProperty("forge_api_url");
    expect(m.root.textContent).toContain("Waiting for the owner of this Clax to approve this repository and installation.");
    expect(m.root.querySelector('[data-token="write"]')).toBeNull();
  });

  it("asks for no installation, and shows the refusal of a repository the App is not on", async () => {
    const message = "the App is not installed on acme/web: install it at https://github.com/apps/clax-bot/installations/new, then try again";
    const refused = [400, { error: { code: "app_not_installed", message } }];
    const { m } = await open({ [`GET ${ROUTE}`]: view({ app: APP }), [`PUT ${ROUTE}`]: refused });
    click(m.root.querySelector('input[name="repo-access"][value="github_app"]'));
    expect(m.root.querySelector('input[name="installation_id"]')).toBeNull();
    type(m.root.querySelector('input[name="repo"]'), "acme/web");
    type(m.root.querySelector('input[name="preview_url"]'), "https://pr-{pr}.acme.dev");
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    expect(m.root.querySelector('form.rform [role="alert"]')!.textContent).toContain("https://github.com/apps/clax-bot/installations/new");
  });

  it("says a token's commits are unsigned, beside the write token", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }) });
    expect(m.root.querySelector('[data-token="write"] [data-unsigned]')!.textContent).toContain("Commits made with a token are unsigned, and a repository that requires signatures will refuse them; use the App.");
  });

  it("states where source goes, by the runtime", async () => {
    const third = await open({ [`GET ${ROUTE}`]: view({ runtime: { ...OWN, third_party: true } }) });
    expect(third.m.root.querySelector('[data-runtime]')!.textContent).toContain("cloned into an ephemeral container on a third-party runtime");
    document.body.replaceChildren();
    const own = await open({ [`GET ${ROUTE}`]: view() });
    expect(own.m.root.querySelector('[data-runtime]')!.textContent).toContain("on this daemon's own runtime");
    expect(own.m.root.querySelector('[data-runtime]')!.textContent).not.toContain("third-party");
    document.body.replaceChildren();
    const none = await open({ [`GET ${ROUTE}`]: view({ runtime: { ...OWN, configured: false } }) });
    expect(none.m.root.querySelector('[data-runtime]')!.textContent).toContain("no container runtime");
  });

  it("offers no link on a Clax that runs its operator's repositories alone", async () => {
    const only = { ...OWN, third_party: true, operator_only: true };
    const none = await open({ [`GET ${ROUTE}`]: view({ runtime: only }) });
    expect(none.m.root.querySelector("[data-runtime]")!.textContent).toContain("runs only the repositories its operator names");
    expect(none.m.root.querySelector("[data-runtime]")!.textContent).not.toContain("is cloned into");
    expect(none.m.root.querySelector("form")).toBeNull();
    document.body.replaceChildren();
    // A link made before stays, to be read and taken away, and says it cannot run.
    const kept = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, status: "ready" }, runtime: only }) });
    expect(kept.m.root.querySelector(".stat")!.textContent).toContain("cannot run");
    expect(kept.m.root.querySelector("form")).not.toBeNull();
    document.body.replaceChildren();
    const theirs = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, status: "ready" }, runtime: { ...only, operator_target: true } }) });
    expect(theirs.m.root.querySelector(".stat")!.textContent).toContain("that one runs.");
  });

  it("prints where a token goes, beside each field", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, repo_url: "https://git.acme.dev/a/b.git", forge_api_url: "https://api.acme.dev" } }) });
    for (const t of ["write", "read"]) expect(m.root.querySelector(`[data-token="${t}"] [data-hosts]`)!.textContent).toBe("Sent to git.acme.dev and api.acme.dev.");
  });

  it("reads the link again when another admin changed it, and shows the new address before any retry", async () => {
    let moved = false;
    const elsewhere = { ...LINK, rev: "r2", repo_url: "https://git.acme.dev/acme/web.git", forge_api_url: "https://git.acme.dev/api" };
    const { calls, m } = await open({
      [`GET ${ROUTE}`]: () => view({ repo: moved ? elsewhere : LINK }),
      [`PUT ${ROUTE}/tokens/write`]: () => { moved = true; return [400, { error: { code: "link_changed", message: "another admin changed this link; read it again" } }]; },
    });
    const write = () => m.root.querySelector('[data-token="write"]')!;
    click(button(write(), "Set"));
    await settle();
    type(write().querySelector('input[type="password"]'), "github_pat_secret_value");
    click(write().querySelector('button[type="submit"]'));
    await settle();
    expect(write().querySelector('[role="alert"]')!.textContent).toBe("Another admin changed this link; read it again.");
    expect(write().querySelector("[data-hosts]")!.textContent).toBe("Sent to git.acme.dev.");
    expect(calls.filter(c => c.method === "GET" && c.url === ROUTE).length).toBe(2);
  });

  it("a stale Save is refused, says so, and shows what the link is now", async () => {
    let moved = false;
    const { calls, m } = await open({
      [`GET ${ROUTE}`]: () => view({ repo: moved ? { ...LINK, rev: "r2", base: "develop" } : LINK }),
      [`PUT ${ROUTE}`]: () => { moved = true; return [400, { error: { code: "link_changed", message: "another admin changed this link; read it again" } }]; },
    });
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    expect((calls.find(c => c.method === "PUT")!.body as { if_rev: string }).if_rev).toBe("r1");
    expect(m.root.querySelector('form.rform [role="alert"]')!.textContent).toContain("Another admin changed this link");
    expect(m.root.querySelector<HTMLInputElement>('input[name="base"]')!.value).toBe("develop");
  });

  it("leaves a URL holding the default empty, so renaming the repository moves it", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }), [`PUT ${ROUTE}`]: view({ repo: LINK }) });
    expect(m.root.querySelector<HTMLInputElement>('input[name="repo_url"]')!.value).toBe("");
    expect(m.root.querySelector<HTMLInputElement>('input[name="forge_api_url"]')!.value).toBe("");
    type(m.root.querySelector('input[name="repo"]'), "acme/renamed");
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    const body = calls.find(c => c.method === "PUT")!.body as Record<string, unknown>;
    expect(body).not.toHaveProperty("repo_url");
    expect(body).not.toHaveProperty("forge_api_url");
  });

  it("opens More settings on a refusal of a field in it", async () => {
    const { m } = await open({
      [`GET ${ROUTE}`]: view({ repo: LINK }),
      [`PUT ${ROUTE}`]: [400, { error: { code: "invalid_repo_url", message: "a URL here is https://" } }],
    });
    const more = m.root.querySelector<HTMLDetailsElement>("details.more")!;
    expect(more.open).toBe(false);
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    expect(m.root.querySelector<HTMLDetailsElement>("details.more")!.open).toBe(true);
  });

  it("says what is true of a ready link: saved, tried, or nothing to run it", async () => {
    const ready = { ...LINK, status: "ready", ci_accepted: true, access: { kind: "token", read_token: SET, write_token: SET } };
    const said = async (o: Record<string, unknown>) => {
      const { m } = await open({ [`GET ${ROUTE}`]: view(o) });
      const text = m.root.querySelector(".stat")!.textContent!;
      document.body.replaceChildren();
      return text;
    };
    expect(await said({ repo: ready })).toContain("Details saved; nothing published yet.");
    expect(await said({ repo: { ...ready, last_ok_at: "2026-11-05T12:00:00Z" } })).toContain("Connected.");
    expect(await said({ repo: ready, runtime: { ...OWN, configured: false } })).toContain("Nothing can use them until this Clax has a container runtime.");
    expect(await said({ repo: ready, runtime: { ...OWN, failed: true, third_party: true } })).toContain("did not start");
    expect(await said({ repo: ready, runtime: { ...OWN, operator_target: true } })).toContain("their own configuration, and that one runs.");
    expect(await said({ repo: ready, runtime: { ...OWN, egress_held: false } })).toContain("needs its containers' network held to an allow-list");
    expect(await said({ repo: { ...ready, allow_open_egress: true }, runtime: { ...OWN, egress_held: false } })).toContain("nothing published yet");
    expect(await said({ repo: { ...ready, public: true }, runtime: { ...OWN, egress_held: false } })).toContain("nothing published yet");
    expect(await said({ repo: { ...ready, last_failure: "The container could not clone the repository." } })).toContain("The last try failed. The container could not clone the repository.");
  });

  it("never reads as finished on a link that cannot run, whatever was tried before", async () => {
    const tried = { ...LINK, last_ok_at: "2026-11-05T12:00:00Z", access: { kind: "token", read_token: UNSET, write_token: SET } };
    const said = async (o: Record<string, unknown>) => {
      const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: { ...tried, ...o } }) });
      const text = m.root.querySelector(".stat")!.textContent!;
      document.body.replaceChildren();
      return text;
    };
    for (const status of ["needs_token", "needs_read_token", "needs_ci_acceptance", "needs_app", "needs_approval"]) {
      const text = await said({ status });
      expect(text).not.toContain("Connected.");
      expect(text).not.toContain("nothing published yet");
    }
    expect(await said({ status: "needs_read_token" })).toContain("Not ready: add a read token below. ClaxBot cannot clone a private repository without one.");
    expect(await said({ status: "needs_ci_acceptance" })).toContain("Not ready: this repository's CI needs your say-so below");
    // A public repository needs no read token, so only the write token is asked for.
    expect(await said({ status: "needs_token", public: true, access: { kind: "token", read_token: UNSET, write_token: UNSET } })).toContain("Not ready: add a write token below.");
  });

  it("explains in plain words what running ClaxBot's branch in CI means, and sends the answer", async () => {
    const waiting = { ...LINK, status: "needs_ci_acceptance", access: { kind: "token", read_token: SET, write_token: SET } };
    const { calls, m } = await open({ [`GET ${ROUTE}`]: view({ repo: waiting }), [`PUT ${ROUTE}`]: view({ repo: { ...waiting, status: "ready", ci_accepted: true } }) });
    const ci = m.root.querySelector("[data-ci]")!;
    expect(ci.closest("details")).toBeNull();
    expect(ci.textContent).toContain("before anyone has read the change");
    expect(ci.textContent).toContain("with the secrets and network access your CI has");
    expect(ci.textContent).toContain("Clax cannot see what your CI does");
    click(m.root.querySelector('input[name="ci_accepted"]'));
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toMatchObject({ ci_accepted: true });
    expect(m.root.querySelector<HTMLInputElement>('input[name="ci_accepted"]')!.checked).toBe(true);
  });

  it("says beside the preview address that with no working preview a fix waits for a person, public or not", async () => {
    for (const repo of [LINK, { ...LINK, public: true }, null]) {
      const { m } = await open({ [`GET ${ROUTE}`]: view({ repo }) });
      const line = m.root.querySelector("[data-no-preview]")!;
      expect(line.closest("details")).toBeNull();
      expect(line.textContent).toBe("With no working preview, ClaxBot's fix waits for a person: the reporter gets nothing until someone hands it over with a link.");
      document.body.replaceChildren();
    }
  });

  it("does not ask about CI for a public repository", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, public: true } }) });
    expect(m.root.querySelector("[data-ci]")).toBeNull();
    expect(m.root.querySelector('input[name="ci_accepted"]')).toBeNull();
  });

  it("says an allow-list holding a host people upload to does not stop source reaching it", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }) });
    const note = m.root.querySelector("[data-allowlist]")!;
    expect(note.closest("details")).toBeNull();
    expect(note.textContent).toContain("a host people can upload to, such as github.com");
    expect(note.textContent).toContain("could still send your source there");
    document.body.replaceChildren();
    const open2 = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, public: true } }) });
    expect(open2.m.root.querySelector("[data-allowlist]")).toBeNull();
  });

  it("keeps what was typed when another admin changed the link, and shows what they changed", async () => {
    let moved = false;
    const { m } = await open({
      [`GET ${ROUTE}`]: () => view({ repo: moved ? { ...LINK, rev: "r2", base: "develop", preview_ready_path: "/ready" } : LINK }),
      [`PUT ${ROUTE}`]: () => { moved = true; return [400, { error: { code: "link_changed", message: "another admin changed this link; read it again" } }]; },
    });
    type(m.root.querySelector('input[name="preview_url"]'), "https://typed-{pr}.acme.dev");
    type(m.root.querySelector('textarea[name="protected_paths"]'), "infra/");
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    const value = (s: string) => m.root.querySelector<HTMLInputElement>(s)!.value;
    expect(value('input[name="preview_url"]')).toBe("https://typed-{pr}.acme.dev");
    expect(value('textarea[name="protected_paths"]')).toBe("infra/");
    expect(value('input[name="base"]')).toBe("develop");
    expect(value('input[name="preview_ready_path"]')).toBe("/ready");
    expect(m.root.querySelector("[data-kept]")!.textContent).toContain("What you typed is kept");
  });

  it("keeps what was typed when a refused token shows the link moved on", async () => {
    let moved = false;
    const { m } = await open({
      [`GET ${ROUTE}`]: () => view({ repo: moved ? { ...LINK, rev: "r2" } : LINK }),
      [`PUT ${ROUTE}/tokens/write`]: () => { moved = true; return [400, { error: { code: "link_changed", message: "another admin changed this link; read it again" } }]; },
    });
    type(m.root.querySelector('input[name="base"]'), "typed");
    const write = () => m.root.querySelector('[data-token="write"]')!;
    click(button(write(), "Set"));
    await settle();
    type(write().querySelector('input[type="password"]'), "github_pat_secret_value");
    click(write().querySelector('button[type="submit"]'));
    await settle();
    expect(m.root.querySelector<HTMLInputElement>('input[name="base"]')!.value).toBe("typed");
  });

  it("keeps what was typed when a token's answer carries a new rev", async () => {
    const taken = { ...LINK, rev: "r2", status: "needs_read_token", access: { kind: "token", read_token: UNSET, write_token: SET } };
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }), [`PUT ${ROUTE}/tokens/write`]: view({ repo: taken }) });
    type(m.root.querySelector('input[name="base"]'), "typed");
    const write = m.root.querySelector('[data-token="write"]')!;
    click(button(write, "Set"));
    await settle();
    type(write.querySelector('input[type="password"]'), "github_pat_secret_value");
    click(write.querySelector('button[type="submit"]'));
    await settle();
    expect(m.root.querySelector<HTMLInputElement>('input[name="base"]')!.value).toBe("typed");
  });

  it("warns that clearing a token stops new runs, and leaves a run under way alone", async () => {
    const asked = vi.fn(() => false);
    vi.stubGlobal("confirm", asked);
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, access: { kind: "token", read_token: UNSET, write_token: SET } } }) });
    click(button(m.root.querySelector('[data-token="write"]')!, "Clear"));
    await settle();
    expect(asked).toHaveBeenCalledWith("Clear the write token? New runs stop using it; a run under way goes on with it.");
  });

  it("says what leaves, and what a pasted token risks", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }) });
    const text = m.root.textContent!;
    expect(text).toContain("What ClaxBot reads of the repository is sent to this Clax's model provider.");
    expect(text).toContain("a pasted token works until it expires, so set an expiry");
    expect(text).toContain("a token that can write, pasted here, goes into the container");
  });

  it("keeps unsaved edits when a token is taken", async () => {
    const taken = { ...LINK, status: "ready", access: { kind: "token", read_token: UNSET, write_token: SET } };
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }), [`PUT ${ROUTE}/tokens/write`]: view({ repo: taken }) });
    type(m.root.querySelector('input[name="base"]'), "develop");
    const write = m.root.querySelector('[data-token="write"]')!;
    click(button(write, "Set"));
    await settle();
    type(write.querySelector('input[type="password"]'), "github_pat_secret_value");
    click(write.querySelector('button[type="submit"]'));
    await settle();
    expect(m.root.querySelector('[data-token="write"]')!.textContent).toContain("Set, ending rite");
    expect(m.root.querySelector<HTMLInputElement>('input[name="base"]')!.value).toBe("develop");
  });

  it("keeps a named forge in its field when the repository is off GitHub", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, repo_url: "https://git.acme.dev/a/b.git" } }) });
    expect(m.root.querySelector<HTMLInputElement>('input[name="forge_api_url"]')!.value).toBe("https://api.github.com");
  });

  it("sends the confidentiality settings, off until asked, and says what each risks", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: view({ repo: LINK }), [`PUT ${ROUTE}`]: view({ repo: { ...LINK, public: true } }) });
    expect(m.root.textContent).toContain("anything steering ClaxBot could send your source anywhere");
    click(m.root.querySelector('input[name="public"]'));
    click(m.root.querySelector('form.rform button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")!.body).toMatchObject({ public: true, allow_open_egress: false, reviewed_previews: false, wake_on_reply: false });
    expect(m.root.querySelector<HTMLInputElement>('input[name="public"]')!.checked).toBe(true);
  });

  it("offers another try when the link cannot be read", async () => {
    let ok = false;
    const { m } = await open({ [`GET ${ROUTE}`]: () => ok ? view() : [500, { error: { code: "internal", message: "try later" } }] });
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("Could not load the repository");
    ok = true;
    click(button(m.root, "Try again"));
    await settle();
    expect(m.root.textContent).toContain("Not connected.");
    expect(m.root.querySelector('[role="alert"]')).toBeNull();
  });

  it("fills the form from the link, and disconnects after confirming", async () => {
    vi.stubGlobal("confirm", vi.fn(() => true));
    const { calls, m } = await open({ [`GET ${ROUTE}`]: view({ repo: { ...LINK, protected_paths: ["infra/", "Makefile"] } }), [`DELETE ${ROUTE}`]: {} });
    expect(m.root.querySelector<HTMLInputElement>('input[name="repo"]')!.value).toBe("acme/web");
    expect(m.root.querySelector<HTMLTextAreaElement>('textarea[name="protected_paths"]')!.value).toBe("infra/\nMakefile");
    click(button(m.root, "Disconnect"));
    await settle();
    expect(calls.find(c => c.method === "DELETE")).toMatchObject({ url: ROUTE, body: { if_rev: "r1" } });
    expect(m.root.textContent).toContain("Not connected.");
  });
});
