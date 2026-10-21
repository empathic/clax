import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settle, stubFetch } from "../people/fixtures";
import { waitFor } from "../test/artifact-view";

beforeEach(() => { vi.resetModules(); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

const person = { public_id: "u_bo", display_name: "Bo", owner: false, guest: false };
const docs = { terms_url: "/terms", privacy_url: "/privacy", version: "2026-10-19" };
const me = (agreed: boolean | null) => ({ origin: "https://clax.example", setup_open: false, signed_in: agreed !== null, person: agreed === null ? null : person, legal: { ...docs, agreed } });
const prompt = () => document.querySelector('[data-testid="legal-prompt"]');

describe("hosted legal documents", () => {
  it("asks a person who has not agreed, and Continue records the version and closes the prompt", async () => {
    const calls = stubFetch({ "GET /api/hosted/me": me(false), "POST /api/hosted/legal/agree": {} });
    const { hostedMe } = await import("./legal");
    expect((await hostedMe())?.legal?.terms_url).toBe("/terms");
    await waitFor(prompt, "the prompt");
    expect(prompt()!.querySelector('a[href="/privacy"]')).not.toBeNull();
    prompt()!.querySelector<HTMLButtonElement>("button.primary")!.click();
    await settle();
    expect(calls.find(c => c.method === "POST")?.body).toEqual({ version: "2026-10-19" });
    expect(prompt(), "the prompt closed").toBeNull();
  });

  it("keeps the prompt and says why when agreeing fails", async () => {
    stubFetch({ "GET /api/hosted/me": me(false), "POST /api/hosted/legal/agree": [500, { error: { code: "internal", message: "the store is unavailable" } }] });
    const { hostedMe } = await import("./legal");
    await hostedMe();
    await waitFor(prompt, "the prompt");
    prompt()!.querySelector<HTMLButtonElement>("button.primary")!.click();
    await settle();
    expect(prompt()!.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("asks nothing of a person who agreed, a signed-out visitor or a local daemon, and asks the daemon once", async () => {
    for (const answer of [me(true), me(null)]) {
      vi.resetModules();
      const calls = stubFetch({ "GET /api/hosted/me": answer });
      const { hostedMe } = await import("./legal");
      await hostedMe();
      await hostedMe();
      await settle();
      expect(prompt()).toBeNull();
      expect(calls.filter(c => c.url === "/api/hosted/me").length).toBe(1);
    }
    vi.resetModules();
    stubFetch({});
    const { hostedMe } = await import("./legal");
    expect(await hostedMe()).toBeNull();
  });
});
