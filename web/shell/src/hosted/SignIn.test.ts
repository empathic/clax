import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import SignIn from "./SignIn.svelte";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); history.replaceState(null, "", "/"); });

const me = { origin: "https://clax.example", setup_open: false, signed_in: true, person: { public_id: "u_sam", display_name: "Sam", owner: false, guest: false } };
const waiting = (kind: string | null) => ({ client_name: "Clax on Pixel", peer_ip: "203.0.113.9", client_kind: kind, created_at: "2026-10-22T10:00:00Z", expires_at: "2026-10-22T10:10:00Z", same_address: true });

async function approvalPage(kind: string | null, owner = false) {
  history.replaceState(null, "", "/device?code=WXYZ-4821");
  stubFetch({
    "GET /api/hosted/me": { ...me, person: { ...me.person, owner } },
    "GET /api/hosted/device/WXYZ-4821": waiting(kind),
  });
  const m = mount(SignIn, {});
  await settle();
  return m.root.textContent ?? "";
}

describe("the device approval page", () => {
  it("tells the person the Clax app may manage the sharing of their apps", async () => {
    const text = await approvalPage("clax-app");
    expect(text).toContain("Clax on Pixel");
    expect(text).toContain("the Clax app");
    expect(text).toContain("It gets an agent token of yours: what it publishes is yours, within your rights, and an app it publishes stays private until you share it.");
    expect(text).not.toContain("public as yours");
    expect(text).toContain("it may also change who can see or edit the pages you made, and whether anyone may comment on them.");
    expect(text).not.toContain("any page on this site");
  });

  it("tells the owner the Clax app may change who can see or edit any page on this site", async () => {
    const text = await approvalPage("clax-app", true);
    expect(text).toContain("change who can see or edit the pages you made and, as the owner, any page on this site");
  });

  it("says another machine's Clax gets an agent token and shares nothing", async () => {
    const text = await approvalPage(null);
    expect(text).toContain("It gets an agent token of yours: what it publishes is public as yours, within your rights.");
    expect(text).not.toContain("the Clax app");
    expect(text).not.toContain("change who can see or edit");
  });
});
