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
