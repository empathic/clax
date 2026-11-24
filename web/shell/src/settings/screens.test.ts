import { describe, expect, it } from "vitest";
import { moved } from "./moved";
import { type Ctx, SCREENS, groupsFor, hrefOf, idOf, screenFor, search } from "./screens";

const ctx = (o: Partial<Ctx> = {}): Ctx => ({ hosted: false, owner: false, member: true, admin: false, bot: false, ...o });
const LOCAL_OWNER = ctx({ owner: true, admin: true, bot: true });
const HOSTED_MEMBER = ctx({ hosted: true, bot: true });
const ids = (c: Ctx) => groupsFor(c).map(g => [g.name, g.screens.map(s => s.id)]);

describe("Settings' screens", () => {
  it("lists a local owner every screen but the hosted ones, in four groups", () => {
    expect(ids(LOCAL_OWNER)).toEqual([
      ["You", ["agents", "claxbot", "confirmations"]],
      ["People", ["people", "roles"]],
      ["ClaxBot", ["provider", "web", "pricing", "usage"]],
      ["System", ["config", "remotes", "audit"]],
    ]);
  });

  it("lists a hosted member their own screens and the audit log, and nothing of the instance's", () => {
    expect(ids(HOSTED_MEMBER)).toEqual([
      ["You", ["account", "agents", "sharing", "claxbot"]],
      ["System", ["audit"]],
    ]);
  });

  it("lists nothing to a browser that is no member's", () => {
    expect(ids(ctx({ member: false }))).toEqual([]);
  });

  it("shows the screen a path names, the one it names instead, or none", () => {
    expect(idOf("/settings")).toBeNull();
    expect(idOf("/settings/people")).toBe("people");
    expect(idOf("/settings/people/x")).toBeNull();
    expect(screenFor("people", LOCAL_OWNER)?.id).toBe("people");
    expect(screenFor("people", HOSTED_MEMBER)?.id).toBe("agents");
    expect(screenFor("account", LOCAL_OWNER)?.id).toBe("agents");
    expect(screenFor("provider", HOSTED_MEMBER)).toBeNull();
    expect(screenFor("nowhere", LOCAL_OWNER)).toBeNull();
    // A page outside Settings is no screen of it.
    expect(screenFor("queues", LOCAL_OWNER)).toBeNull();
  });

  it("gives every screen and item an address, and no two screens one ID", () => {
    expect(new Set(SCREENS.map(s => s.id)).size).toBe(SCREENS.length);
    const people = SCREENS.find(s => s.id === "people")!;
    expect(hrefOf(people)).toBe("/settings/people");
    expect(hrefOf(people, "?invite")).toBe("/settings/people?invite");
    expect(hrefOf(SCREENS.find(s => s.id === "queues")!, "#x")).toBe("/queues");
  });
});

describe("the headings Settings' items name", () => {
  // Every `#<ID>` an item goes to is an ID some screen's source gives a
  // heading. Configuration's are its tables' (`sec-<table>`), made from data.
  const sources = Object.values(import.meta.glob("../{people,hosted,bot,q}/*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>).join("\n");

  it("are IDs the screens' sources carry", () => {
    const missing = SCREENS.flatMap(s => s.items.map(i => i.at ?? ""))
      .filter(at => at.startsWith("#") && !at.startsWith("#sec-"))
      .filter(at => !sources.includes(`id="${at.slice(1)}"`));
    expect(missing).toEqual([]);
    expect(sources).toContain("id={`sec-${sec.name}`}");
    expect(sources).toContain('id="sec-remotes"');
  });
});

describe("Settings' search", () => {
  const titles = (q: string, c: Ctx) => search(q, c).map(h => h.title);

  it("lists the screens this browser sees before anything is typed", () => {
    expect(titles("", HOSTED_MEMBER)).toEqual(["Account", "Agents & machines", "Shared links", "Your ClaxBot", "Audit log", "Tags"]);
  });

  it("finds an item by its title, saying where it is and going to its heading", () => {
    const [first] = search("agent tok", HOSTED_MEMBER);
    expect(first).toMatchObject({ title: "Agent tokens", where: "You › Agents & machines", href: "/settings/agents#ht-h" });
  });

  it("finds an item by words it never shows", () => {
    expect(titles("api key", HOSTED_MEMBER)).toContain("Agent tokens");
    expect(titles("log out", HOSTED_MEMBER)).toEqual(["Sign out"]);
    expect(titles("webauthn", LOCAL_OWNER)).toEqual(["Require a passkey to confirm"]);
  });

  it("finds nothing on a screen the browser may not see, nor an item that is not there", () => {
    expect(titles("passkey", HOSTED_MEMBER)).toEqual([]);
    expect(titles("invite", HOSTED_MEMBER)).toEqual([]);
    // A local Clax has no agent tokens.
    expect(titles("agent tokens", LOCAL_OWNER)).not.toContain("Agent tokens");
    expect(titles("cloudflare", ctx({ hosted: true, owner: true, admin: true }))).toEqual([]);
  });

  it("leads with a title that starts with the query, then one holding a word that does", () => {
    const t = titles("usage", LOCAL_OWNER);
    expect(t.slice(0, 3)).toEqual(["Usage", "Usage by person", "Usage by cost line"]);
    expect(t).toContain("Credits this week");
  });

  it("takes every word of the query, in any order and any case", () => {
    expect(titles("PERSON invite", LOCAL_OWNER)).toEqual(["Invite a person"]);
    expect(titles("invite zebra", LOCAL_OWNER)).toEqual([]);
  });

  it("finds a model key's failed requests: the instance key's for an admin, a person's own key's for them", () => {
    expect(search("failures", LOCAL_OWNER)[0]).toMatchObject({ title: "Failures", where: "ClaxBot › Model provider", href: "/settings/provider#pv-f" });
    expect(titles("failed request", LOCAL_OWNER)).toEqual(expect.arrayContaining(["Failures", "The key's last failed request", "Your key's last failed request"]));
    expect(search("key failed", HOSTED_MEMBER).map(h => h.href)).toEqual(["/settings/claxbot#me-k"]);
  });

  it("finds the queue settings that live on the Queues page, for those who see it", () => {
    expect(search("service grant", LOCAL_OWNER)[0]).toMatchObject({ title: "Service grants", where: "Elsewhere › Queues", href: "/queues" });
    expect(titles("service grant", HOSTED_MEMBER)).toEqual([]);
  });
});

describe("the pages that moved into Settings", () => {
  it("go to their screens, keeping their query", () => {
    expect(moved("/people")).toBe("/settings/people");
    expect(moved("/people", "?invite")).toBe("/settings/people?invite=");
    expect(moved("/me")).toBe("/settings/claxbot");
    expect(moved("/audit", "?actor=claxbot&range=7d")).toBe("/settings/audit?actor=claxbot&range=7d");
    expect(moved("/admin")).toBe("/settings/roles");
    expect(moved("/admin", "?tab=pricing")).toBe("/settings/pricing");
    expect(moved("/admin", "?tab=usage&week=2026-11-02")).toBe("/settings/usage?week=2026-11-02");
    expect(moved("/admin", "?tab=nowhere")).toBe("/settings/roles");
  });

  it("are no other path", () => {
    for (const p of ["/", "/settings", "/settings/people", "/queues", "/inbox", "/tags"]) expect(moved(p)).toBeNull();
  });
});
