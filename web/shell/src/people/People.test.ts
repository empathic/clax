import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { agent, COLLAB, grant, person, settle, stubFetch } from "./fixtures";
import { mount } from "../test/svelte";
import People from "./People.svelte";

const OWNER_ME = { person: { public_id: "o1", display_name: "Alex", owner: true, rights: COLLAB }, grants: [], agents: [] };
const mia = person({ public_id: "p2", display_name: "Mia", grants: [grant({ id: "g9", client: { name: "mia-pc", key: "SHA256:mm", peer_ip: "192.168.1.9" }, person: { public_id: "p2", display_name: "Mia" } })], agents: [agent({ handle: "h9", name: "mia-agent", person: "p2", grant_id: "g9" })] });
const ownerRoutes = (o: Record<string, unknown> = {}) => ({
  "GET /api/people/me": OWNER_ME,
  "GET /api/people": { people: [person(), mia, person({ public_id: "p3", display_name: "Old", revoked_at: "2026-10-01T00:00:00Z", grants: [], agents: [] })], unconnected: { on: true, approval_id: "ap1" } },
  "GET /api/people/invites": { invites: [] },
  "GET /api/grants?status=pending": { grants: [grant({ id: "g5", status: "pending", via: "request", person: null, client: { name: "new-box", key: "SHA256:nn", peer_ip: "192.168.1.77" } })] },
  "GET /api/approvals": { approvals: [{ id: "a1", approver: null, author: { public_id: "p1", name: "Sam" }, scope: "everywhere", artifact_id: null, thread_id: null, created_at: "", created_via: "people", request_id: null }] },
  "GET /api/approvals/requests": { requests: [], open: 0, banner: null },
  ...o,
});

beforeEach(() => { history.replaceState(null, "", "/people"); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("People", () => {
  it("shows the owner everyone, the switch, connection requests and invites, with the revoked as history", async () => {
    stubFetch(ownerRoutes());
    const m = mount(People, {});
    await settle();
    expect([...m.root.querySelectorAll(".person > header h3")].map(h => h.textContent!.trim())).toEqual(expect.arrayContaining(["Sam", "Mia"]));
    expect(m.root.querySelector('[role="switch"]')!.getAttribute("aria-checked")).toBe("true");
    expect(m.root.querySelector(".requests")!.textContent).toContain("new-box");
    expect(m.root.querySelector(".requests")!.textContent).toContain("SHA256:nn");
    expect(m.root.querySelector(".revoked")!.textContent).toContain("Old");
    // Sam's content reaches the owner's agents: listed with Revoke.
    expect(m.root.querySelector('[data-person="p1"] .approvals')!.textContent).toContain("everywhere");
    expect(document.title).toBe("People · Clax");
  });

  it("shows the switch once while the first invite's dialog is open, in the dialog", async () => {
    let invites: unknown[] = [];
    stubFetch(ownerRoutes({
      "GET /api/people/invites": () => ({ invites }),
      "POST /api/people/invites": () => { invites = [{ id: "i1", person: { public_id: "p9", display_name: "Kim" }, owner: false, created_at: "", expires_at: "2099-01-01T00:00:00Z", used_at: null, grant_id: null, revoked_at: null }]; return { invite: { id: "i1", command: "clax connect http://192.168.1.5:7480/i/K7QF-M2XD-9PLA", expires_at: "2099-01-01T00:00:00Z", person: { public_id: "p9", display_name: "Kim" } } }; },
    }));
    const m = mount(People, {});
    await settle();
    expect(m.root.querySelectorAll('[role="switch"]')).toHaveLength(1);
    const { flush } = await import("../test/svelte");
    flush(() => { [...m.root.querySelectorAll("button")].find(b => b.textContent === "Invite a person")!.click(); });
    const input = m.root.querySelector<HTMLInputElement>('.invite input[name="name"]')!;
    flush(() => { input.value = "Kim"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => { m.root.querySelector(".invite form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    await settle();
    expect(m.root.querySelector(".invite code")).not.toBeNull();
    expect(m.root.querySelectorAll('[role="switch"]')).toHaveLength(1);
    expect(m.root.querySelector('.invite [role="switch"]')).not.toBeNull();
  });

  it("shows a connected person only themselves, their machines and their agents", async () => {
    const calls = stubFetch({
      "GET /api/people/me": { person: { public_id: "p1", display_name: "Sam", owner: false, rights: COLLAB }, grants: [grant()], agents: [agent()] },
      "GET /api/approvals": { approvals: [{ id: "a2", approver: "p1", author: { public_id: "o1", name: "Alex" }, scope: "everywhere", artifact_id: null, thread_id: null, created_at: "", created_via: "connect", request_id: null }] },
      "GET /api/approvals/requests": { requests: [], open: 0, banner: null },
    });
    const m = mount(People, {});
    await settle();
    expect([...m.root.querySelectorAll(".person")]).toHaveLength(1);
    expect(m.root.querySelector(".person h3")!.textContent).toContain("Sam");
    expect(m.root.textContent).not.toContain("Mia");
    expect(m.root.querySelector('[role="switch"]')).toBeNull();
    expect([...m.root.querySelectorAll("button")].some(b => b.textContent === "Invite a person")).toBe(false);
    expect(calls.map(c => c.url)).not.toContain("/api/people");
    expect(calls.map(c => c.url)).not.toContain("/api/people/invites");
    // Their approvals: the owner's content reaching their agents.
    expect(m.root.querySelector(".approvals")!.textContent).toContain("Alex");
  });

  it("refetches when the people topic says someone connected, and stops following it when it goes", async () => {
    let people = [person()];
    stubFetch(ownerRoutes({ "GET /api/people": () => ({ people, unconnected: { on: true, approval_id: "ap1" } }) }));
    let hear: ((e: { type: string }) => void) | null = null;
    const off = vi.fn();
    const watch = vi.fn((_t: string[], on: (e: { type: string }) => void) => { hear = on; return off; });
    const m = mount(People, { stream: { watch } as unknown as import("../stream").EventStream });
    await settle();
    expect(watch.mock.calls[0][0]).toEqual(["people", "agents"]);
    expect(m.root.textContent).not.toContain("Mia");
    people = [person(), mia];
    hear!({ type: "people", topic: "people" } as { type: string });
    await settle();
    expect(m.root.querySelector('[data-person="p2"] h3')!.textContent).toBe("Mia");
    m.unmount();
    expect(off).toHaveBeenCalled();
  });

  it("tells anyone else that People is the owner's and connected people's", async () => {
    stubFetch({ "GET /api/people/me": [403, { error: { code: "forbidden", message: "only the owner or a connected person has People" } }] });
    const m = mount(People, {});
    await settle();
    expect(m.root.querySelector(".person")).toBeNull();
    expect(m.root.textContent).toContain("only the owner or a connected person has People");
  });
});
