import { afterEach, describe, expect, it, vi } from "vitest";
import { settle, stubFetch } from "../people/fixtures";
import { mount } from "../test/svelte";
import Debugger from "./Debugger.svelte";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

const view = (root: Element, h: string) => [...root.querySelectorAll("section section")].find(s => s.querySelector("h4")?.textContent === h)!;

describe("Debugger", () => {
  it("shows each view from its route, says why one refuses, and names the seat building what is not there", async () => {
    stubFetch({
      "GET /healthz": { version: "0.9.0", pid: 41, started_at: new Date(Date.now() - 90 * 60_000).toISOString(), commit: "cbd7eebcd0123456", build: null },
      "GET /api/toolpath/status": { journal: true, dir: "/h/journal", segment: "s1", cursor: 3, newest_seq: 3, lag_ms: 0, last_error: null, warning: null },
      "GET /api/extension": [403, { error: { code: "forbidden", message: "the owner's token only" } }],
      "GET /api/people/me": { person: { public_id: "p1", display_name: "Alex", owner: false }, grants: [{ id: "g1", status: "approved", via: "pair", client: { name: "basis-queue", peer_ip: null }, person: null, owner: true, created_at: "t", last_used_at: "2026-11-02T22:00:00Z", ended_at: null }], agents: [] },
      "GET /api/live/sites": { sites: [{ site: { key: "k", name: "phobos", joined: true, origins: ["https://phobos.example", "http://localhost:7700"] }, pages: 1, threads: 0, last_activity: null, joining: [] }] },
      "GET /api/audit?group=bot": { events: [{ seq: 9, at: "2026-11-02T21:00:00Z", actor: { display_name: "ClaxBot" }, on_behalf_of: null, kind: "bot.run.start", summary: "started a run", artifact_id: "a", run: "r1" }], next_cursor: null },
      "GET /api/audit?q=agent_sent": { events: [], next_cursor: null },
    });
    const m = mount(Debugger, {});
    await settle();
    const status = view(m.root, "Status").textContent!;
    expect(status).toContain("0.9.0 (cbd7eebcd012)");
    expect(status).toContain("1h 30m");
    expect(status).toContain("on, at /h/journal");
    expect(view(m.root, "Status").querySelector('[role="alert"]')!.textContent).toBe("the owner's token only");
    expect(status).toContain("not the owner");
    expect(status).toContain("owner_tailscale_nodes");
    expect(view(m.root, "Sites and joins").textContent).toContain("phobos, joined: https://phobos.example, http://localhost:7700");
    expect(view(m.root, "Sites and joins").querySelector(".nyb")!.textContent).toBe("Not built yet — work-clax-unjoin-stint: unjoining a joined site");
    expect(view(m.root, "Queues and responders").textContent).toContain("basis-queue (approved, via pair)");
    expect(view(m.root, "ClaxBot runs").textContent).toContain("run r1 · ClaxBot: started a run");
    expect(view(m.root, "Agent delivery").textContent).toContain("No Send-to-agent outcome");
    // No control that does nothing: every button is a live one.
    expect([...m.root.querySelectorAll("button")].map(b => b.textContent)).toEqual([]);
  });
});
