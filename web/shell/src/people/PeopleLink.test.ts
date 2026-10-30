import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "./fixtures";
import PeopleLink from "./PeopleLink.svelte";
import { forget } from "../bot/me";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); forget(); });

describe("PeopleLink", () => {
  it("goes when the People page finds a person revoked, once the daemon says so", async () => {
    const calls = stubFetch({ "GET /api/viewers/me": { viewer: { public_id: "p1", display_name: "Sam", person: true, person_revoked_at: "2026-10-12T10:00:00Z" } } });
    const m = mount(PeopleLink, { owner: false, viewer: { public_id: "p1", display_name: "Sam", person: true } });
    expect(m.root.querySelector("a.people-link")).not.toBeNull();
    dispatchEvent(new Event("clax:people-gone"));
    await settle();
    // No longer a person, it asks whether a hosted daemon's owner holds the
    // browser; a local daemon answers 404.
    expect(calls.map(c => c.url).filter(u => u === "/api/viewers/me" || u === "/api/hosted/me")).toEqual(["/api/viewers/me", "/api/hosted/me"]);
    expect(m.root.querySelector("a.people-link")).toBeNull();
  });

  it("shows for a hosted daemon's owner, whose browser holds no token, and not for its guests", async () => {
    stubFetch({ "GET /api/hosted/me": { origin: "https://clax.example", setup_open: false, signed_in: true, person: { public_id: "o1", display_name: "Ada", owner: true, guest: false } } });
    const owner = mount(PeopleLink, { owner: false, viewer: { public_id: "o1", display_name: "Ada" } });
    await settle();
    expect(owner.root.querySelector("a.people-link")).not.toBeNull();
    document.body.replaceChildren();
    stubFetch({ "GET /api/hosted/me": { origin: "https://clax.example", setup_open: false, signed_in: true, person: { public_id: "g1", display_name: "Mia", owner: false, guest: true } } });
    const guest = mount(PeopleLink, { owner: false, viewer: { public_id: "g1", display_name: "Mia" } });
    await settle();
    expect(guest.root.querySelector("a.people-link")).toBeNull();
  });

  it("shows Queues to the owner and the instance's admins, and not to a member", async () => {
    stubFetch({});
    const owner = mount(PeopleLink, { owner: true, viewer: null });
    await settle();
    expect(owner.root.querySelector("a.queues-link")).not.toBeNull();
    document.body.replaceChildren();
    forget();
    stubFetch({ "GET /api/me/bot": { enabled: false, role: "admin" } });
    const admin = mount(PeopleLink, { owner: false, viewer: { public_id: "a1", display_name: "Ada", person: true } });
    await settle();
    expect(admin.root.querySelector("a.people-link")).not.toBeNull();
    expect(admin.root.querySelector("a.queues-link")).not.toBeNull();
    document.body.replaceChildren();
    forget();
    stubFetch({ "GET /api/me/bot": { enabled: false, role: "member" } });
    const member = mount(PeopleLink, { owner: false, viewer: { public_id: "m1", display_name: "Mia", person: true } });
    await settle();
    expect(member.root.querySelector("a.people-link")).not.toBeNull();
    expect(member.root.querySelector("a.queues-link")).toBeNull();
  });
});
