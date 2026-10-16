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
});
