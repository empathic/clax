import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "./fixtures";
import PeopleLink from "./PeopleLink.svelte";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("PeopleLink", () => {
  it("goes when the People page finds a person revoked, once the daemon says so", async () => {
    const calls = stubFetch({ "GET /api/viewers/me": { viewer: { public_id: "p1", display_name: "Sam", person: true, person_revoked_at: "2026-10-12T10:00:00Z" } } });
    const m = mount(PeopleLink, { owner: false, viewer: { public_id: "p1", display_name: "Sam", person: true } });
    expect(m.root.querySelector("a.people-link")).not.toBeNull();
    dispatchEvent(new Event("clax:people-gone"));
    await settle();
    expect(calls.map(c => c.url)).toEqual(["/api/viewers/me"]);
    expect(m.root.querySelector("a.people-link")).toBeNull();
  });
});
