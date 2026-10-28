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
function setup(a: Record<string, unknown>) {
  const calls: { method: string; url: string }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    calls.push({ method: init.method ?? "GET", url });
    return new Response(JSON.stringify(init.method === "DELETE" ? { withdrawn: true } : a), { status: 200 });
  }));
  const ctl = {
    id: ID,
    state: writable({ data: { artifact: { title: "Forecast", may_approve: true, pending_capabilities: a.pending ?? null } }, approving: false }),
    host: { grantState: async () => null, revoke: async () => {} },
    approve: vi.fn(async () => {}), notNow: vi.fn(), showApproval: vi.fn(),
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
});
