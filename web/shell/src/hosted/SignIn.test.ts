import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { nav } from "../nav";
import SignIn from "./SignIn.svelte";

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); document.body.replaceChildren(); history.replaceState(null, "", "/"); });

const me = { origin: "https://clax.example", setup_open: false, signed_in: true, person: { public_id: "u_sam", display_name: "Sam", owner: false, guest: false } };
const waiting = (kind: string | null) => ({ client_name: "Clax on Pixel", peer_ip: "203.0.113.9", client_kind: kind, created_at: "2026-10-22T10:00:00Z", expires_at: "2026-10-22T10:10:00Z", same_address: true });

async function approvalPage(kind: string | null, owner = false, return_to: string | null = null) {
  history.replaceState(null, "", "/device?code=WXYZ-4821");
  stubFetch({
    "GET /api/hosted/me": { ...me, person: { ...me.person, owner } },
    "GET /api/hosted/device/WXYZ-4821": { ...waiting(kind), return_to },
    "POST /api/hosted/device/WXYZ-4821/approve": { token: { id: "t1" } },
    "POST /api/hosted/device/WXYZ-4821/deny": {},
  });
  const m = mount(SignIn, {});
  await settle();
  return m;
}

describe("the device approval page", () => {
  it("tells the person the Clax app may manage the sharing of their apps", async () => {
    const text = (await approvalPage("clax-app")).root.textContent ?? "";
    expect(text).toContain("Clax on Pixel");
    expect(text).toContain("the Clax app");
    expect(text).toContain("It gets an agent token of yours: what it publishes is yours, within your rights, and an app it publishes stays private until you share it.");
    expect(text).not.toContain("public as yours");
    expect(text).toContain("it may also change who can see or edit the pages you made, and whether anyone may comment on them.");
    expect(text).not.toContain("any page on this site");
  });

  it("tells the owner the Clax app may change who can see or edit any page on this site", async () => {
    const text = (await approvalPage("clax-app", true)).root.textContent ?? "";
    expect(text).toContain("change who can see or edit the pages you made and, as the owner, any page on this site");
  });

  it("says another machine's Clax gets an agent token and shares nothing", async () => {
    const text = (await approvalPage(null)).root.textContent ?? "";
    expect(text).toContain("It gets an agent token of yours: what it publishes is public as yours, within your rights.");
    expect(text).not.toContain("the Clax app");
    expect(text).not.toContain("change who can see or edit");
  });
});

describe("returning to the Clax that asked", () => {
  const to = "http://localhost:7480/a/x?publish=1";
  const press = async (m: Awaited<ReturnType<typeof approvalPage>>, label: string) => {
    [...m.root.querySelectorAll("button")].find(b => b.textContent === label)!.click();
    await settle();
  };

  it("approving_returns_to_the_loopback_page", async () => {
    const assign = vi.spyOn(nav, "assign").mockImplementation(() => {});
    const m = await approvalPage(null, false, to);
    expect(m.root.textContent).toContain("go back to localhost:7480");
    await press(m, "Approve");
    expect(assign).toHaveBeenCalledWith("http://localhost:7480/a/x?publish=1&clax_signin=approved");
  });

  it("denying_returns_too", async () => {
    const assign = vi.spyOn(nav, "assign").mockImplementation(() => {});
    const m = await approvalPage(null, false, to);
    await press(m, "Deny");
    expect(assign).toHaveBeenCalledWith("http://localhost:7480/a/x?publish=1&clax_signin=denied");
  });

  it("keeps_close_this_tab_without_a_return_address", async () => {
    const assign = vi.spyOn(nav, "assign").mockImplementation(() => {});
    const m = await approvalPage(null, false, null);
    expect(m.root.textContent).not.toContain("go back to");
    await press(m, "Approve");
    expect(m.root.textContent).toContain("You can close this tab.");
    expect(assign).not.toHaveBeenCalled();
  });

  it("ignores_a_return_address_that_is_not_loopback", async () => {
    const assign = vi.spyOn(nav, "assign").mockImplementation(() => {});
    const m = await approvalPage(null, false, "http://evil.example/");
    await press(m, "Approve");
    expect(assign).not.toHaveBeenCalled();
  });
});
