import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "./fixtures";
import MemberLinks from "./MemberLinks.svelte";
import { forget } from "../bot/me";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); forget(); });

describe("MemberLinks", () => {
  it("takes Settings away when a screen finds a person revoked, once the daemon says so", async () => {
    const calls = stubFetch({ "GET /api/viewers/me": { viewer: { public_id: "p1", display_name: "Sam", person: true, person_revoked_at: "2026-10-12T10:00:00Z" } } });
    const m = mount(MemberLinks, { show: "settings", owner: false, viewer: { public_id: "p1", display_name: "Sam", person: true } });
    expect(m.root.querySelector("a.settings-link")).not.toBeNull();
    dispatchEvent(new Event("clax:people-gone"));
    await settle();
    // No longer a person, it asks whether a hosted daemon's owner holds the
    // browser; a local daemon answers 404.
    expect(calls.map(c => c.url).filter(u => u === "/api/viewers/me" || u === "/api/hosted/me")).toEqual(["/api/viewers/me", "/api/hosted/me"]);
    expect(m.root.querySelector("a.settings-link")).toBeNull();
  });

  it("shows for a hosted daemon's owner, whose browser holds no token, and not for its guests", async () => {
    stubFetch({ "GET /api/hosted/me": { origin: "https://clax.example", setup_open: false, signed_in: true, person: { public_id: "o1", display_name: "Ada", owner: true, guest: false } } });
    const owner = mount(MemberLinks, { show: "settings", owner: false, viewer: { public_id: "o1", display_name: "Ada" } });
    await settle();
    expect(owner.root.querySelector("a.settings-link")).not.toBeNull();
    document.body.replaceChildren();
    stubFetch({ "GET /api/hosted/me": { origin: "https://clax.example", setup_open: false, signed_in: true, person: { public_id: "g1", display_name: "Mia", owner: false, guest: true } } });
    const guest = mount(MemberLinks, { show: "settings", owner: false, viewer: { public_id: "g1", display_name: "Mia" } });
    await settle();
    expect(guest.root.querySelector("a.settings-link")).toBeNull();
  });

  it("shows Queues to the owner and the instance's admins, and not to a member", async () => {
    stubFetch({});
    const owner = mount(MemberLinks, { show: "queues", owner: true, viewer: null });
    await settle();
    expect(owner.root.querySelector("a.queues-link")).not.toBeNull();
    document.body.replaceChildren();
    forget();
    stubFetch({ "GET /api/me/bot": { enabled: false, role: "admin" } });
    const admin = mount(MemberLinks, { show: "queues", owner: false, viewer: { public_id: "a1", display_name: "Ada", person: true } });
    await settle();
    expect(admin.root.querySelector("a.queues-link")).not.toBeNull();
    document.body.replaceChildren();
    forget();
    stubFetch({ "GET /api/me/bot": { enabled: false, role: "member" } });
    const member = mount(MemberLinks, { show: "queues", owner: false, viewer: { public_id: "m1", display_name: "Mia", person: true } });
    await settle();
    expect(member.root.querySelector("a.queues-link")).toBeNull();
  });

  it("is a gear linking to Settings, marked current there, and opens Settings' search on the Settings key", async () => {
    stubFetch({});
    const m = mount(MemberLinks, { show: "settings", current: "settings", owner: true, viewer: null });
    await settle();
    const gear = m.root.querySelector<HTMLAnchorElement>("a.settings-link")!;
    expect(gear.getAttribute("href")).toBe("/settings");
    expect(gear.getAttribute("aria-label")).toBe("Settings");
    expect(gear.getAttribute("aria-current")).toBe("page");
    expect(m.root.querySelector("a.queues-link")).toBeNull();
    const key = new KeyboardEvent("keydown", { key: "k", metaKey: true, cancelable: true });
    dispatchEvent(key);
    expect(key.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(document.querySelector("[role=dialog][aria-label='Search settings']")).not.toBeNull());
  });

  it("leaves the Settings key alone for a browser that is no member's", async () => {
    stubFetch({});
    mount(MemberLinks, { show: "settings", owner: false, viewer: null });
    await settle();
    const key = new KeyboardEvent("keydown", { key: "k", metaKey: true, cancelable: true });
    dispatchEvent(key);
    expect(key.defaultPrevented).toBe(false);
  });
});
