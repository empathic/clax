import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import HostedPanel from "./HostedPanel.svelte";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

const me = { origin: "https://clax.example", setup_open: false, signed_in: true, person: { public_id: "u_sam", display_name: "Sam", owner: false, guest: false } };

describe("HostedPanel", () => {
  it("shows a refusal as People's other surfaces do: a sentence, not the daemon's lower-case message", async () => {
    stubFetch({
      "GET /api/hosted/agent-tokens": { tokens: [] },
      "GET /api/hosted/links": { links: [] },
      "GET /api/artifacts": { artifacts: [] },
      "POST /api/hosted/agent-tokens": [429, { error: { code: "rate_limited", message: "too many tokens made from this address" } }],
    });
    const m = mount(HostedPanel, { me });
    await settle();
    const form = m.root.querySelector<HTMLFormElement>("form.mint")!;
    flush(() => { form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Too many tokens made from this address.");
    expect(m.root.querySelector('.hosted-tokens [role="alert"]'), "the refusal shows under Agent tokens").not.toBeNull();
    expect(m.root.querySelectorAll('[role="alert"]').length).toBe(1);
  });

  it("says a taken name in People's own words", async () => {
    stubFetch({
      "GET /api/hosted/agent-tokens": { tokens: [] },
      "GET /api/hosted/links": { links: [] },
      "GET /api/artifacts": { artifacts: [] },
      "POST /api/hosted/agent-tokens": [409, { error: { code: "name_taken", message: "that label is taken" } }],
    });
    const m = mount(HostedPanel, { me });
    await settle();
    flush(() => { m.root.querySelector("form.mint")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("That name is taken, or reads too like a name already here: choose another.");
  });

  it("shows a new token's commands for Claude Code, Codex and Grok Build", async () => {
    const mcp = "https://clax.example/mcp/json";
    stubFetch({
      "GET /api/hosted/agent-tokens": { tokens: [] },
      "GET /api/hosted/links": { links: [] },
      "GET /api/artifacts": { artifacts: [] },
      "POST /api/hosted/agent-tokens": [201, {
        id: "g1", label: "laptop", created_at: "2026-10-27T10:00:00Z", token: "cxa_t", mcp_url: mcp,
        claude_command: `claude mcp add --transport http clax ${mcp} --header "Authorization: Bearer cxa_t"`,
        codex_config: `[mcp_servers.clax]\nurl = "${mcp}"\n`,
        grok_command: `grok mcp add --transport http clax_hosted ${mcp} --header "Authorization: Bearer cxa_t"`,
      }],
    });
    const m = mount(HostedPanel, { me });
    await settle();
    flush(() => { m.root.querySelector("form.mint")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    await settle();
    const minted = m.root.querySelector(".minted")!;
    expect(minted.textContent).toContain("Or to Grok Build:");
    expect([...minted.querySelectorAll(".cmd code")].map(c => c.textContent)).toEqual([
      `claude mcp add --transport http clax ${mcp} --header "Authorization: Bearer cxa_t"`,
      `grok mcp add --transport http clax_hosted ${mcp} --header "Authorization: Bearer cxa_t"`,
    ]);
  });

  it("lists the person's devices apart from their agent tokens", async () => {
    const tok = (id: string, label: string, kind: "device" | "agent") => ({ id, label, status: "approved", created_at: "2026-10-20T10:00:00Z", last_used_at: null, ended_at: null, kind });
    stubFetch({
      "GET /api/hosted/agent-tokens": { tokens: [tok("g2", "Clax on Pixel", "device"), tok("g1", "laptop", "agent")] },
      "GET /api/hosted/links": { links: [] },
      "GET /api/artifacts": { artifacts: [] },
    });
    const m = mount(HostedPanel, { me });
    await settle();
    const names = (sel: string) => [...m.root.querySelectorAll(`${sel} .mname`)].map(e => e.textContent);
    expect(m.root.querySelector(".hosted-devices h3")!.textContent).toBe("Devices");
    expect(names(".hosted-devices")).toEqual(["Clax on Pixel"]);
    expect(m.root.querySelector(".hosted-devices .note")!.textContent).toContain("Each holds an agent token of yours, with your rights, and may also change who can see or edit the pages you made, and whether anyone may comment on them.");
    expect(names(".hosted-tokens")).toEqual(["laptop"]);
  });

  it("sets a token's queue access from its row, at once, and shows what others granted", async () => {
    const grants = {
      grant_id: "g1",
      raises: [
        { granter: { public_id: "u_sam", name: "Sam", owner: false }, access: [{ type: "clax-queues", actions: ["read"], locations: ["q1"] }], version: "v1", created_at: "2026-11-17T10:00:00Z" },
        { granter: { public_id: "u_mia", name: "Mia", owner: false }, access: [{ type: "clax-queues", actions: ["claim", "read"], locations: ["*"] }], version: "v2", created_at: "2026-11-17T11:00:00Z" },
      ],
      queues: [],
    };
    const calls = stubFetch({
      "GET /api/hosted/agent-tokens": { tokens: [{ id: "g1", label: "laptop", status: "approved", created_at: "2026-11-17T10:00:00Z", last_used_at: null, ended_at: null, kind: "agent", grants }] },
      "GET /api/hosted/links": { links: [] },
      "GET /api/artifacts": { artifacts: [] },
      "GET /api/queues": { queues: [{ id: "q1", name: "App reports" }, { id: "q2", name: "Billing" }] },
      "PUT /api/people/grants/g1/elevate": { grant: grants },
      "DELETE /api/people/grants/g1/elevate": { grant: grants },
      "DELETE /api/people/grants/g1/elevate?granter=u_mia": { grant: grants },
    });
    const m = mount(HostedPanel, { me });
    await settle();
    const row = m.root.querySelector(".hosted-tokens .tok-queues")!;
    expect(row.querySelector("summary")!.textContent).toBe("Queues Read on App reports");
    expect(row.querySelector(".granted")!.textContent).toContain("Mia grants Read, Claim on all my queues");
    const box = (name: string, value: string) => row.querySelector<HTMLInputElement>(`input[name="${name}"][value="${value}"]`)!;
    expect(box("queue", "q1").checked).toBe(true);
    expect(box("action", "claim").checked).toBe(false);
    // Turning Claim on sends the whole grant at once.
    flush(() => { box("action", "claim").click(); });
    await settle();
    const put = calls.find(c => c.method === "PUT")!;
    expect(put.url).toBe("/api/people/grants/g1/elevate");
    expect(put.body).toEqual({ access: [{ type: "clax-queues", actions: ["read", "claim"], locations: ["q1"] }] });
    // Turning Read off leaves nothing: the grant is removed.
    flush(() => { box("action", "read").click(); });
    await settle();
    expect(calls.some(c => c.method === "DELETE" && c.url === "/api/people/grants/g1/elevate")).toBe(true);
    // Mia's grant on Sam's token is Sam's to remove.
    flush(() => { row.querySelector<HTMLButtonElement>(".granted button")!.click(); });
    await settle();
    expect(calls.some(c => c.method === "DELETE" && c.url === "/api/people/grants/g1/elevate?granter=u_mia")).toBe(true);
  });

  it("grants a new token the queue access chosen in the Create token form", async () => {
    const calls = stubFetch({
      "GET /api/hosted/agent-tokens": { tokens: [] },
      "GET /api/hosted/links": { links: [] },
      "GET /api/artifacts": { artifacts: [] },
      "GET /api/queues": { queues: [{ id: "q1", name: "App reports" }] },
      "POST /api/hosted/agent-tokens": [201, { id: "g9", label: "laptop", created_at: "2026-11-17T10:00:00Z", token: "cxa_t", mcp_url: "https://clax.example/mcp/json", claude_command: "claude", codex_config: "codex" }],
      "PUT /api/people/grants/g9/elevate": { grant: { grant_id: "g9", raises: [] } },
    });
    const m = mount(HostedPanel, { me });
    await settle();
    const form = m.root.querySelector<HTMLFormElement>("form.mint")!;
    expect(form.querySelector("summary")!.textContent).toBe("Queues No queue access");
    flush(() => { form.querySelector<HTMLInputElement>('input[name="all"]')!.click(); });
    flush(() => { form.querySelector<HTMLInputElement>('input[name="action"][value="resolve"]')!.click(); });
    expect(form.querySelector("summary")!.textContent).toBe("Queues Read, Resolve on all my queues");
    flush(() => { form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    await settle();
    const put = calls.find(c => c.method === "PUT")!;
    expect(put.url).toBe("/api/people/grants/g9/elevate");
    expect(put.body).toEqual({ access: [{ type: "clax-queues", actions: ["read", "resolve"], locations: ["*"] }] });
    expect(calls.findIndex(c => c.method === "POST")).toBeLessThan(calls.findIndex(c => c.method === "PUT"));
    // The form starts again at none.
    expect(form.querySelector("summary")!.textContent).toBe("Queues No queue access");
  });

  it("shows no Devices part without a device", async () => {
    stubFetch({
      "GET /api/hosted/agent-tokens": { tokens: [] },
      "GET /api/hosted/links": { links: [] },
      "GET /api/artifacts": { artifacts: [] },
    });
    const m = mount(HostedPanel, { me });
    await settle();
    expect(m.root.querySelector(".hosted-devices")).toBeNull();
  });
});
