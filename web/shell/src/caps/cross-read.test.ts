import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { dbHandler } from "./db";
import type { CapEnv } from "./host";
import type { Prompt, PromptAnswer } from "./grants";

const A = "7q3k9mzx2b4t";
const B = "8r4m0nay3c5v";
const C = "9s5n1pbz4d6w";
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

type Req = { method: string; url: string; auth: string | null };

/** The owner's daemon as the shell reaches it: A and B's titles and
 * declarations, and A's grants. */
function setup(opts: { token?: string | null; declared?: Record<string, unknown>; answer?: PromptAnswer; sourceDb?: boolean; grantFails?: boolean } = {}) {
  const requests: Req[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const h = (init.headers ?? {}) as Record<string, string>;
    const method = init.method ?? "GET";
    requests.push({ method, url, auth: h.authorization ?? null });
    if (method === "PUT" && url === `/api/artifacts/${A}/db-grants/${B}`) {
      return opts.grantFails ? json({ error: { code: "internal", message: "no" } }, 500) : json({ grant: { source_id: B, granted_at: "now" } });
    }
    if (url === `/api/artifacts/${A}`) return json({ artifact: { id: A, title: "Dashboard" } });
    if (url === `/api/artifacts/${B}`) return json({ artifact: { id: B, title: "Runs", capabilities: opts.sourceDb === false ? {} : { db: {} } } });
    return json({ error: { code: "not_found", message: "no" } }, 404);
  }));
  const prompts: Prompt[] = [];
  const prompt = vi.fn(async (p: Prompt) => { prompts.push(p); return opts.answer ?? "allow"; });
  const env = {
    aid: A, version: 1, pinned: false, token: opts.token === undefined ? "tok" : opts.token,
    declared: opts.declared ?? { db: { read: [B] } }, prompt, post: (_m: ShellToBridge) => {},
  } as unknown as CapEnv;
  return { h: dbHandler(env, null as never), requests, prompts };
}

describe("db.artifact(id)'s consent in the shell", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("asks the owner once per source, and saves the grant on Allow", async () => {
    const { h, requests, prompts } = setup();
    const [first, second] = await Promise.all([h.call("grant", [B]), h.call("grant", [B])]);
    expect([first, second]).toEqual([true, true]);
    expect(prompts).toEqual([{ kind: "crossRead", title: "Let “Dashboard” read “Runs”'s data?", body: expect.stringContaining("cannot change"), allow: "Allow", deny: "Don't allow", always: "Always for this app" }]);
    const put = requests.filter(r => r.method === "PUT");
    expect(put).toEqual([{ method: "PUT", url: `/api/artifacts/${A}/db-grants/${B}`, auth: "Bearer tok" }]);
  });

  it("asks again when a call finds the grant it saved revoked", async () => {
    const { h, prompts } = setup();
    expect(await h.call("grant", [B])).toBe(true);
    // The page's next read was refused not_granted: the grant is gone.
    expect(await h.call("grant", [B])).toBe(true);
    expect(prompts).toHaveLength(2);
  });

  it("answers false on Don't allow, which stands for the view", async () => {
    const { h, prompts, requests } = setup({ answer: "deny" });
    expect(await h.call("grant", [B])).toBe(false);
    expect(await h.call("grant", [B])).toBe(false);
    expect(prompts).toHaveLength(1);
    expect(requests.some(r => r.method === "PUT")).toBe(false);
  });

  it("asks again after a dismissed prompt", async () => {
    const { h, prompts } = setup({ answer: "dismiss" });
    expect(await h.call("grant", [B])).toBe(false);
    expect(await h.call("grant", [B])).toBe(false);
    expect(prompts).toHaveLength(2);
  });

  it("asks nothing of a viewer who is not the owner", async () => {
    const { h, prompts, requests } = setup({ token: null });
    expect(await h.call("grant", [B])).toBe(false);
    expect(prompts).toEqual([]);
    expect(requests).toEqual([]);
  });

  it("refuses a source the app does not declare, or that is no artifact ID, before any request", async () => {
    const { h, requests } = setup();
    await expect(h.call("grant", [C])).rejects.toMatchObject({ code: "not_declared" });
    await expect(h.call("grant", [A])).rejects.toMatchObject({ code: "not_declared" });
    await expect(h.call("grant", ["../x"])).rejects.toMatchObject({ code: "invalid_argument" });
    expect(requests).toEqual([]);
  });

  it("asks nothing for a source that keeps no documents", async () => {
    const { h, prompts } = setup({ sourceDb: false });
    await expect(h.call("grant", [B])).rejects.toMatchObject({ code: "not_declared" });
    expect(prompts).toEqual([]);
  });

  it("says when the grant could not be saved, and asks again next time", async () => {
    const { h, prompts } = setup({ grantFails: true });
    await expect(h.call("grant", [B])).rejects.toMatchObject({ code: "unavailable" });
    await expect(h.call("grant", [B])).rejects.toMatchObject({ code: "unavailable" });
    expect(prompts).toHaveLength(2);
  });
});
