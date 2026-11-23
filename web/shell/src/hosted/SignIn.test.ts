import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { nav } from "../nav";
import SignIn from "./SignIn.svelte";
import type { SignInBoot } from "./signin-boot";
import { SIGNIN_SKELETON_HTML } from "../view/gallery-frame";

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
    expect(m.root.textContent).toContain("Approve, then go back to localhost:7480.");
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

describe("the device page signed out", () => {
  const signedOut = async (github: boolean) => {
    history.replaceState(null, "", "/device?code=WXYZ-4821");
    stubFetch({ "GET /api/hosted/me": { origin: "https://clax.example", setup_open: false, signed_in: false, person: null, github } });
    const m = mount(SignIn, {});
    await settle();
    return m;
  };

  it("offers_github_sign_in_that_returns_to_the_code", async () => {
    const m = await signedOut(true);
    const a = m.root.querySelector<HTMLAnchorElement>("[data-testid=github-signin]");
    expect(a?.getAttribute("href")).toBe("/auth/github?next=%2Fdevice%3Fcode%3DWXYZ-4821");
    expect(m.root.textContent).toContain("Another Clax asks to publish here as you. Sign in first");
  });

  it("asks_for_a_sign_in_link_without_github", async () => {
    const m = await signedOut(false);
    expect(m.root.querySelector("[data-testid=github-signin]")).toBeNull();
    expect(m.root.textContent).toContain("open a sign-in link from a device that is signed in");
  });
});

describe("the first render", () => {
  const signedOut = { origin: "https://clax.example", setup_open: false, signed_in: false, person: null, github: true, legal: { terms_url: "/terms", privacy_url: "/privacy", version: "1", agreed: null } };
  const block = (o: Partial<SignInBoot>): SignInBoot => ({ v: 1, kind: "signin", status: {}, hosted: null, public: null, invite: null, link: null, device: null, ...o });

  it("is the settled page from the daemon's block, asking nothing", () => {
    history.replaceState(null, "", "/signin");
    const calls = stubFetch({});
    const m = mount(SignIn, { boot: block({ status: { hosted: 200, public: 200 }, hosted: signedOut, public: { artifacts: [{ artifact_id: "a1", title: "Roadmap", url: "https://clax.example/a/a1" }] } }) });
    expect(m.root.querySelector("main.signin")!.hasAttribute("aria-busy")).toBe(false);
    expect(m.root.querySelector("[data-testid=github-signin]")).not.toBeNull();
    expect(m.root.querySelector("[data-testid=legal-notice]")).not.toBeNull();
    expect(m.root.querySelector(".pub li a")?.textContent).toBe("Roadmap");
    expect(m.root.querySelector(".silegal")?.textContent).toBe("Terms of Service · Privacy Policy · Source on GitHub");
    expect(calls).toEqual([]);
  });

  it("names an invite the block read, and says a code the block found spent", () => {
    history.replaceState(null, "", "/join/k9");
    stubFetch({});
    let m = mount(SignIn, { boot: block({ status: { hosted: 200, invite: 200 }, hosted: signedOut, invite: { name: "Mia", expires_at: "2026-11-14T00:00:00Z" } }) });
    expect([...m.root.querySelectorAll(".sicard button")].map(b => b.textContent)).toEqual(["Join as Mia"]);
    m.unmount();
    m = mount(SignIn, { boot: block({ status: { hosted: 200, invite: 404 }, hosted: signedOut }) });
    expect(m.root.textContent).toContain("This invite is not valid.");
  });

  it("shows the frame's skeleton until every answer is in, then the page once", async () => {
    history.replaceState(null, "", "/join/k9");
    let answer!: (r: Response) => void;
    vi.stubGlobal("fetch", vi.fn(async (url: string) => url === "/api/hosted/me" ? new Response(JSON.stringify(signedOut)) : new Promise<Response>(r => { answer = r; })));
    const m = mount(SignIn, {});
    await settle();
    expect(m.root.querySelector("main.signin")!.getAttribute("aria-busy")).toBe("true");
    expect(m.root.querySelector(".sicard")!.innerHTML).toContain(SIGNIN_SKELETON_HTML);
    expect(m.root.querySelector(".sicard > p, .silegal a")).toBeNull();
    answer(new Response(JSON.stringify({ name: "Mia", expires_at: "2026-11-14T00:00:00Z" })));
    await settle();
    expect(m.root.querySelector("main.signin")!.hasAttribute("aria-busy")).toBe(false);
    expect(m.root.querySelector(".sk")).toBeNull();
    expect(m.root.textContent).toContain("You have been invited as Mia.");
  });

  it("keeps a pressed control in place, busy, until its answer, and says so", async () => {
    history.replaceState(null, "", "/join/k9");
    let answer!: (r: Response) => void;
    vi.stubGlobal("fetch", vi.fn((url: string) => url.endsWith("/accept") ? new Promise<Response>(r => { answer = r; }) : Promise.resolve(new Response("{}"))));
    const m = mount(SignIn, { boot: block({ status: { hosted: 200, invite: 200 }, hosted: signedOut, invite: { name: "Mia", expires_at: "2026-11-14T00:00:00Z" } }) });
    const b = m.root.querySelector<HTMLButtonElement>("button.primary")!;
    b.click();
    await settle();
    expect(b.getAttribute("aria-busy")).toBe("true");
    expect(b.disabled).toBe(true);
    expect(b.textContent).toBe("Join as Mia");
    expect(m.root.querySelector(".vh[role=status]")?.textContent).toBe("Joining…");
    answer(new Response(JSON.stringify({ error: { code: "invite_spent", message: "That invite was used." } }), { status: 410 }));
    await settle();
    expect(b.getAttribute("aria-busy")).toBe("false");
    expect(b.disabled).toBe(false);
    expect(m.root.querySelector("[role=alert]")?.textContent).toBe("That invite was used.");
  });
});
