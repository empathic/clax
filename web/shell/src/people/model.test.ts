import { describe, expect, it } from "vitest";
import type { PeopleAgent, PeopleGrant, Rights } from "../api";
import { CAPS, agentCaps, elevation, expiresIn, liveInvites, machineCaps, narrowAgent, narrowGrant, narrowerOrEqual, presetOf, PRESETS, raiseCaps } from "./model";

const COLLAB = PRESETS["clax:collaborator"];
const key = (type: string, action: string) => `${type} ${action}`;
const DELETE = key("clax-artifacts", "delete");
const SEND = key("clax-comments", "send");
const PUBLISH = key("clax-artifacts", "update");

const grant = (narrow: Rights | null = null): PeopleGrant => ({
  id: "g1", status: "approved", via: "invite", client: { name: "sam-mbp", key: "SHA256:9f2c", peer_ip: "192.168.1.40" },
  person: { public_id: "p1", display_name: "Sam" }, owner: false, narrow, created_at: "2026-10-09T10:00:00Z",
  decided_at: "2026-10-09T10:00:00Z", last_used_at: null, ended_at: null, ended_by: null,
});
const agent = (o: Partial<PeopleAgent> = {}): PeopleAgent => ({
  handle: "h1", name: "clax-multi-person", harness: "codex", person: "p1", grant_id: "g1", started_at: "2026-10-09T10:01:00Z",
  narrow: null, elevate: null, rights: COLLAB, ...o,
});
const on = (rows: { key: string; on: boolean }[]) => rows.filter(r => r.on).map(r => r.key);

describe("people model", () => {
  it("names the preset a person's rights are, in whatever grouping the daemon merged them into", () => {
    expect(presetOf(COLLAB)).toBe("clax:collaborator");
    expect(presetOf(PRESETS["clax:reader"])).toBe("clax:reader");
    // The daemon's merged form of the full preset.
    expect(presetOf([
      { type: "clax-artifacts", actions: ["create", "delete", "read", "update"], locations: ["*"] },
      { type: "clax-comments", actions: ["read", "reply", "resolve", "send", "watch", "write"], locations: ["*"] },
      { type: "clax-db", actions: ["read", "write"], locations: ["*"], level: "admin" },
      { type: "clax-ask", actions: ["ask"] },
    ])).toBe("clax:full");
    // Collaborator less delete on its own artifacts is no preset.
    expect(presetOf(COLLAB.map(a => (a.type === "clax-artifacts" ? { ...a, actions: a.actions.filter(x => x !== "delete") } : a)))).toBeNull();
  });

  it("compares rights as the daemon does: actions, locations and db levels", () => {
    expect(narrowerOrEqual(PRESETS["clax:reader"], COLLAB)).toBe(true);
    expect(narrowerOrEqual(COLLAB, PRESETS["clax:reader"])).toBe(false);
    expect(narrowerOrEqual([{ type: "clax-artifacts", actions: ["update"], locations: ["own:01J9"] }], [{ type: "clax-artifacts", actions: ["update"], locations: ["own"] }])).toBe(true);
    expect(narrowerOrEqual([{ type: "clax-artifacts", actions: ["update"], locations: ["own:01J9"] }], [{ type: "clax-artifacts", actions: ["update"], locations: ["01J9"] }])).toBe(true);
    expect(narrowerOrEqual([{ type: "clax-artifacts", actions: ["update"], locations: ["01J9"] }], [{ type: "clax-artifacts", actions: ["update"], locations: ["own"] }])).toBe(false);
    expect(narrowerOrEqual([{ type: "clax-db", actions: ["write"], locations: ["*"], level: "admin" }], [{ type: "clax-db", actions: ["write"], locations: ["*"], level: "interact" }])).toBe(false);
  });

  it("turns a machine's capability off and on as a narrowing of its person's rights, null when nothing is off", () => {
    expect(on(machineCaps(COLLAB, grant()))).toContain(DELETE);
    const off = narrowGrant(COLLAB, grant(), DELETE, false)!;
    expect(narrowerOrEqual(off, COLLAB)).toBe(true);
    expect(on(machineCaps(COLLAB, grant(off)))).not.toContain(DELETE);
    expect(on(machineCaps(COLLAB, grant(off))).length).toBe(on(machineCaps(COLLAB, grant())).length - 1);
    expect(narrowGrant(COLLAB, grant(off), DELETE, true)).toBeNull();
    // Only what the person may do is offered.
    expect(machineCaps(PRESETS["clax:reader"], grant()).map(r => r.key)).not.toContain(DELETE);
  });

  it("narrows one agent within its machine's narrowing, and says which capabilities the machine has off", () => {
    const g = grant(narrowGrant(COLLAB, grant(), DELETE, false));
    const rows = agentCaps(COLLAB, g, agent());
    expect(rows.find(r => r.key === DELETE)).toMatchObject({ on: false, machineOff: true });
    const n = narrowAgent(COLLAB, g, agent(), SEND, false)!;
    // Within the person's rights and the machine's narrowing both.
    expect(narrowerOrEqual(n, g.narrow!)).toBe(true);
    expect(on(agentCaps(COLLAB, g, agent({ narrow: n })))).not.toContain(SEND);
    expect(on(agentCaps(COLLAB, g, agent({ narrow: n })))).not.toContain(DELETE);
    // Back on: nothing beyond the machine's narrowing is off, so none of its own.
    expect(narrowAgent(COLLAB, g, agent({ narrow: n }), SEND, true)).toBeNull();
  });

  it("raises an agent beyond its person's rights with only what is raised, at every artifact", () => {
    const reader = PRESETS["clax:reader"];
    const a = agent({ rights: reader });
    const rows = raiseCaps(reader, grant(), a);
    expect(rows.find(r => r.key === key("clax-artifacts", "read"))).toMatchObject({ on: true, has: true });
    expect(rows.find(r => r.key === PUBLISH)).toMatchObject({ on: false, has: false });
    const up = elevation(reader, grant(), a, PUBLISH, true)!;
    expect(up).toEqual([{ type: "clax-artifacts", actions: ["update"], locations: ["*"] }]);
    expect(raiseCaps(reader, grant(), agent({ rights: reader, elevate: up })).find(r => r.key === PUBLISH)).toMatchObject({ on: true, has: false });
    expect(elevation(reader, grant(), agent({ elevate: up }), PUBLISH, false)).toBeNull();
    // A db write is raised at interact; ask takes no locations.
    expect(elevation(reader, grant(), a, key("clax-db", "write"), true)).toEqual([{ type: "clax-db", actions: ["write"], locations: ["*"], level: "interact" }]);
    expect(CAPS.map(c => c.key)).toContain(key("clax-ask", "ask"));
  });

  it("keeps the invites that can still be used, and says when one expires", () => {
    const now = new Date("2026-10-09T10:00:00Z");
    const base = { person: { public_id: "p1", display_name: "Sam" }, owner: false, created_at: "2026-10-09T09:00:00Z", grant_id: null };
    const live = { ...base, id: "i1", expires_at: "2026-10-10T09:00:00Z", used_at: null, revoked_at: null };
    expect(liveInvites([
      live,
      { ...live, id: "used", used_at: "2026-10-09T09:30:00Z" },
      { ...live, id: "revoked", revoked_at: "2026-10-09T09:30:00Z" },
      { ...live, id: "old", expires_at: "2026-10-09T09:59:59Z" },
    ], now).map(i => i.id)).toEqual(["i1"]);
    expect(expiresIn("2026-10-10T10:00:00Z", now)).toBe("24 h");
    expect(expiresIn("2026-10-09T10:40:00Z", now)).toBe("40 min");
    expect(expiresIn("2026-10-09T09:00:00Z", now)).toBe("now");
  });
});
