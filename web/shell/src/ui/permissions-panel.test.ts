import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { writable } from "svelte/store";
import PermissionsPanel from "./PermissionsPanel.svelte";
import { permissionsOpen } from "./more-menu.svelte";

let token: string | null = "tok";
vi.mock("../api", async orig => ({ ...(await orig<typeof import("../api")>()), getToken: () => Promise.resolve(token) }));

const ID = "7q3k9mzx2b4t";
const answer = (over: Record<string, unknown> = {}) => ({
  version: 3, declared: {}, in_force: { shell: {} },
  approved: { version: 3, capabilities: { shell: {} }, approved_at: "2026-10-01T00:00:00Z", by: "owner" },
  pending: null,
  net: { declared: ["api.example.com"], waiting: [], grants: [{ host: "api.example.com", granted_at: "2026-10-02T12:00:00Z" }], granted: true },
  sample: { granted: false, granted_at: null },
  db_read: [],
  ...over,
});

const mounted: object[] = [];
type Policy = { content_security_policy?: "self" | "unrestricted"; may_tag?: boolean; patch?: (body: unknown) => Response };
function setup(a: Record<string, unknown>, policy: Policy = {}) {
  const calls: { method: string; url: string; body?: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const body = typeof init.body === "string" ? JSON.parse(init.body) : undefined;
    calls.push({ method: init.method ?? "GET", url, body });
    if (init.method === "PATCH") return policy.patch?.(body) ?? new Response(JSON.stringify({ artifact: { content_security_policy: (body as { content_security_policy: string }).content_security_policy } }), { status: 200 });
    return new Response(JSON.stringify(init.method === "DELETE" ? { withdrawn: true } : a), { status: 200 });
  }));
  const state = writable({ data: { artifact: { title: "Forecast", may_approve: true, pending_capabilities: a.pending ?? null, content_security_policy: policy.content_security_policy, may_tag: policy.may_tag } }, approving: false, policy: null as string | null });
  const ctl = {
    id: ID,
    state,
    host: { grantState: async () => null, revoke: async () => {} },
    approve: vi.fn(async () => {}), notNow: vi.fn(), showApproval: vi.fn(),
    policyChanged: vi.fn((p: string) => state.update(s => ({ ...s, policy: p }))),
  };
  const target = document.createElement("div");
  document.body.append(target);
  mounted.push(mount(PermissionsPanel, { target, props: { ctl: ctl as never } }));
  flushSync();
  return { ctl, calls };
}
afterEach(() => { for (const m of mounted.splice(0)) unmount(m); document.body.replaceChildren(); vi.unstubAllGlobals(); sessionStorage.clear(); token = "tok"; });
const button = (name: string) => [...document.querySelectorAll("button")].find(b => b.textContent === name);

describe("PermissionsPanel", () => {
  it("opens on the press, its rows' places a busy skeleton hidden from assistive technology until they are read", async () => {
    setup(answer());
    let release!: () => void;
    const held = new Promise<void>(r => { release = r; });
    const real = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => { await held; return real(url, init); }));
    permissionsOpen.current!();
    flushSync();
    const panel = document.querySelector<HTMLElement>(".perm-panel")!;
    expect(panel).not.toBeNull();
    expect(panel.getAttribute("aria-busy")).toBe("true");
    expect(panel.querySelector(".perm-sk")!.getAttribute("aria-hidden")).toBe("true");
    expect(panel.textContent).not.toContain("Loading");
    release();
    await vi.waitFor(() => expect(document.querySelector("[data-permission=net]")).not.toBeNull());
    expect(panel.querySelector(".perm-sk")).toBeNull();
    expect(panel.hasAttribute("aria-busy")).toBe(false);
  });

  it("a_member_without_the_owner_token_sees_grant_rows_without_revoke", async () => {
    token = null;
    setup(answer());
    permissionsOpen.current!();
    await vi.waitFor(() => expect(document.querySelector("[data-permission=net]")).not.toBeNull());
    expect(document.querySelector("[data-permission=net]")!.textContent).not.toContain("Revoke");
    // The approval's withdraw needs no token.
    expect(document.querySelector("[data-permission='approved:shell']")!.textContent).toContain("Revoke");
  });

  it("the_waiting_row_opens_the_approval_dialog", async () => {
    const pending = { version: 3, capabilities: ["shell"], hosts: [], declaration_sha256: "a".repeat(64) };
    const { ctl } = setup(answer({ pending, approved: null, in_force: {} }));
    permissionsOpen.current!();
    await vi.waitFor(() => expect(button("Review")).toBeDefined());
    button("Review")!.click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Let “Forecast” use these?"));
    expect(ctl.showApproval).toHaveBeenCalled();
  });

  it("withdraw_sets_the_reopen_flag_and_the_panel_reopens_on_the_next_mount", async () => {
    const { calls } = setup(answer());
    permissionsOpen.current!();
    await vi.waitFor(() => expect(document.querySelector("[data-permission='approved:shell']")).not.toBeNull());
    (document.querySelector("[data-permission='approved:shell'] button") as HTMLButtonElement).click();
    await vi.waitFor(() => expect(calls.some(c => c.method === "DELETE" && c.url === `/api/artifacts/${ID}/capability-approval/shell`)).toBe(true));
    expect(sessionStorage.getItem(`clax.permissions.reopen:${ID}`)).toBe("1");
    for (const m of mounted.splice(0)) unmount(m);
    document.body.replaceChildren();
    setup(answer());
    await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).not.toBeNull());
    expect(sessionStorage.getItem(`clax.permissions.reopen:${ID}`)).toBeNull();
  });

  it("gives whoever administers it the Content Security Policy setting, which explains itself before it sends unrestricted, and returns to self at once", async () => {
    const { ctl, calls } = setup(answer(), { content_security_policy: "self", may_tag: true });
    permissionsOpen.current!("policy");
    const box = await vi.waitFor(() => { const b = document.querySelector<HTMLButtonElement>("button[name=content_security_policy]"); expect(b).not.toBeNull(); return b!; });
    // Opened from the blocked notice, the setting has focus.
    await vi.waitFor(() => expect(document.activeElement).toBe(box));
    expect(document.querySelector("button[name=content_security_policy]")!.textContent).toBe("Serve without Clax's Content Security Policy…");
    expect(document.querySelector(".policy")!.textContent).toContain("Held to its own site: its pages load and connect to it alone.");
    const patches = () => calls.filter(c => c.method === "PATCH");
    flushSync(() => box.click());
    const ask = await vi.waitFor(() => { const a = document.querySelector("[data-testid=csp-confirm]"); expect(a).not.toBeNull(); return a!; });
    expect(ask.textContent).toContain("Clax then sends no Content-Security-Policy holding its pages to their own site: they may load code from, and send anything they can read to, any site.");
    expect(document.querySelector("button[name=content_security_policy]")!.textContent).toBe("Serve without Clax's Content Security Policy…");
    expect(patches()).toEqual([]);
    flushSync(() => button("Confirm")!.click());
    await vi.waitFor(() => expect(ctl.policyChanged).toHaveBeenCalledWith("unrestricted"));
    expect(patches()).toEqual([{ method: "PATCH", url: `/api/artifacts/${ID}`, body: { content_security_policy: "unrestricted" } }]);
    await vi.waitFor(() => expect(document.querySelector("[data-testid=csp-note]")?.textContent).toBe("Served without Clax's Content Security Policy: its pages load and send where they choose."));
    expect(document.querySelector("button[name=content_security_policy]")!.textContent).toBe("Restore Clax's Content Security Policy");
    flushSync(() => box.click());
    await vi.waitFor(() => expect(ctl.policyChanged).toHaveBeenCalledWith("self"));
    expect(document.querySelector("[data-testid=csp-confirm]")).toBeNull();
    expect(patches()[1]).toEqual({ method: "PATCH", url: `/api/artifacts/${ID}`, body: { content_security_policy: "self" } });
  });

  it("says why a change of the Content Security Policy did not take, and leaves the setting as the daemon holds it", async () => {
    const refused = () => new Response(JSON.stringify({ error: { code: "admin_only", message: "only the page's maker sets its site's Content Security Policy" } }), { status: 403 });
    const { ctl } = setup(answer(), { content_security_policy: "self", may_tag: true, patch: refused });
    permissionsOpen.current!();
    const box = await vi.waitFor(() => { const b = document.querySelector<HTMLButtonElement>("button[name=content_security_policy]"); expect(b).not.toBeNull(); return b!; });
    flushSync(() => box.click());
    await vi.waitFor(() => expect(button("Confirm")).toBeDefined());
    flushSync(() => button("Confirm")!.click());
    await vi.waitFor(() => expect(document.querySelector("[role=alert]")?.textContent).toBe("Could not change the Content Security Policy: only the page's maker sets its site's Content Security Policy"));
    expect(document.querySelector("button[name=content_security_policy]")!.textContent).toBe("Serve without Clax's Content Security Policy…");
    expect(ctl.policyChanged).not.toHaveBeenCalled();
  });

  it("shows anyone who does not administer it no control: a line while it is unrestricted, nothing while it is self", async () => {
    setup(answer(), { content_security_policy: "unrestricted", may_tag: false });
    permissionsOpen.current!();
    await vi.waitFor(() => expect(document.querySelector("[data-testid=csp-note]")?.textContent).toBe("Served without Clax's Content Security Policy: its pages load and send where they choose."));
    expect(document.querySelector("button[name=content_security_policy]")).toBeNull();
    for (const m of mounted.splice(0)) unmount(m);
    document.body.replaceChildren();
    setup(answer(), { content_security_policy: "self", may_tag: false });
    permissionsOpen.current!();
    await vi.waitFor(() => expect(document.querySelector("[data-permission=net]")).not.toBeNull());
    expect(document.querySelector(".policy")).toBeNull();
    // A daemon that says nothing of it shows nothing of it.
    for (const m of mounted.splice(0)) unmount(m);
    document.body.replaceChildren();
    setup(answer(), { may_tag: true });
    permissionsOpen.current!();
    await vi.waitFor(() => expect(document.querySelector("[data-permission=net]")).not.toBeNull());
    expect(document.querySelector(".policy")).toBeNull();
  });
});
