import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { Grants, type Prompt, type PromptAnswer } from "./grants";
import type { CapEnv } from "./host";
import { sampleHandler } from "./sample";

class MemoryStorage { data = new Map<string, string>(); getItem(k: string) { return this.data.get(k) ?? null; } setItem(k: string, v: string) { this.data.set(k, v); } }

function sseBody(frames: [string, unknown][]) {
  return frames.map(([e, d]) => `event: ${e}\ndata: ${JSON.stringify(d)}\n\n`).join("");
}

function setup(answer: PromptAnswer = "allow") {
  const posted: ShellToBridge[] = [];
  const counts: number[] = [];
  const ask = vi.fn(async (_p: Prompt) => answer);
  const status = { available: true, provider: "stub", limits: { maxPromptBytes: 65536, tools: { maxCount: 16 } }, calls_today: 0, daily_call_cap: null };
  const env = { aid: "7q3k9mzx2b4t", token: "tk", post: (m: ShellToBridge) => posted.push(m), sampleStatus: async () => status, onSampleCalls: (n: number) => counts.push(n) } as unknown as CapEnv;
  const grants = new Grants("k", new MemoryStorage() as unknown as Storage, { sample: {} }, true, ask, { sample: true });
  const handler = sampleHandler(env, grants);
  const frames = () => posted.filter(m => m.type === "clax:event").map(m => (m as { data: { call: string; event: string; data: unknown } }).data);
  return { handler, ask, frames, counts };
}

afterEach(() => { vi.unstubAllGlobals(); });

describe("sample handler", () => {
  it("asks once, posts the call, relays its frames, and reports the count", async () => {
    const fetchMock = vi.fn(async () => new Response(sseBody([["start", { call_id: "c1", cached: false, calls_today: 3, daily_call_cap: null }], ["text", { delta: "hi" }], ["done", { text: "hi", truncated: false, model_tier_applied: "default" }]]), { headers: { "content-type": "text/event-stream" } }));
    vi.stubGlobal("fetch", fetchMock);
    const { handler, ask, frames, counts } = setup();
    await handler.call("run", ["s1", { input: "q" }]);
    await handler.call("run", ["s2", { input: "q" }]);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(ask.mock.calls[0][0]).toMatchObject({ body: expect.stringContaining("Anthropic API key") });
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/artifacts/7q3k9mzx2b4t/sample");
    expect(new Headers(init.headers).get("authorization")).toBe("Bearer tk");
    expect(JSON.parse(String(init.body))).toEqual({ input: "q" });
    expect(frames().filter(f => f.call === "s1").map(f => f.event)).toEqual(["start", "text", "done"]);
    expect(counts).toEqual([3, 3]);
  });

  it("a declined viewer gets not_granted and nothing is sent", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const { handler } = setup("deny");
    await expect(handler.call("run", ["s1", { input: "q" }])).rejects.toMatchObject({ code: "not_granted" });
    await expect(handler.call("run", ["s2", { input: "q" }])).rejects.toMatchObject({ code: "not_granted" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("a refusal before the stream rejects with the daemon's code", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "rate_limited", message: "cap" } }), { status: 429 })));
    const { handler } = setup();
    await expect(handler.call("run", ["s1", { input: "q" }])).rejects.toMatchObject({ code: "rate_limited", message: "cap" });
  });

  it("a stream that stops without done or error ends upstream_error", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(sseBody([["start", { call_id: "c", cached: false, calls_today: 1 }], ["text", { delta: "par" }]]))));
    const { handler, frames } = setup();
    await handler.call("run", ["s1", { input: "q" }]);
    expect(frames().at(-1)).toMatchObject({ call: "s1", event: "error", data: { code: "upstream_error" } });
  });

  it("cancel aborts the request; a call cancelled during consent is never sent", async () => {
    let seen: AbortSignal | undefined;
    vi.stubGlobal("fetch", vi.fn((_u: string, init: RequestInit) => { seen = init.signal ?? undefined; return new Promise(() => {}); }));
    const { handler } = setup();
    void handler.call("run", ["s1", { input: "q" }]);
    await vi.waitFor(() => expect(seen).toBeDefined());
    await handler.call("cancel", ["s1"]);
    expect(seen!.aborted).toBe(true);

    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    let answer!: (a: PromptAnswer) => void;
    const posted: ShellToBridge[] = [];
    const grants = new Grants("k2", null, { sample: {} }, true, () => new Promise(r => { answer = r; }), { sample: true });
    const h = sampleHandler({ aid: "7q3k9mzx2b4t", token: "tk", post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv, grants);
    const p = h.call("run", ["s9", { input: "q" }]);
    await vi.waitFor(() => expect(answer).toBeTypeOf("function"));
    await h.call("cancel", ["s9"]);
    answer("allow");
    await expect(p).resolves.toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("tool results go to the daemon", async () => {
    const fetchMock = vi.fn(async () => new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);
    const { handler } = setup();
    await handler.call("toolResult", ["01JCALL", "toolu_1", "teal", false]);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/artifacts/7q3k9mzx2b4t/sample/01JCALL/tool_result");
    expect(JSON.parse(String(init.body))).toEqual({ id: "toolu_1", content: "teal", is_error: false });
  });

  it("limits come from the status the shell fetched", async () => {
    const { handler } = setup();
    await expect(handler.call("limits", [])).resolves.toEqual({ maxPromptBytes: 65536, tools: { maxCount: 16 } });
  });
  it("tool results use the daemon's call ID from the start frame", async () => {
    const fetchMock = vi.fn(async (url: string) => (url.endsWith("/sample")
      ? new Response(sseBody([["start", { call_id: "01JDAEMON", cached: false, calls_today: 1 }], ["tool_call", { id: "toolu_1", name: "t", input: {} }]]))
      : new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetchMock);
    const { handler } = setup();
    await handler.call("run", ["s1", { input: "q" }]);
    await handler.call("toolResult", ["s1", "toolu_1", "teal", false]);
    expect(fetchMock.mock.calls.at(-1)![0]).toBe("/api/artifacts/7q3k9mzx2b4t/sample/01JDAEMON/tool_result");
  });

  it("maps the daemon's refusals into the contract's codes", async () => {
    const { handler } = setup();
    for (const [status, code, want] of [
      [401, "unauthorized", "session_expired"],
      [403, "forbidden", "upstream_error"],
      [403, "forbidden_origin", "upstream_error"],
      [404, "not_found", "not_declared"],
      [408, "timeout", "upstream_error"],
      [413, "payload_too_large", "prompt_too_large"],
      [403, "not_declared", "not_declared"],
      [429, "rate_limited", "rate_limited"],
    ] as const) {
      vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code, message: "m" } }), { status })));
      await expect(handler.call("run", [`s${status}${code}`, { input: "q" }]), `${status} ${code}`).rejects.toMatchObject({ code: want });
    }
  });

  it("leave aborts every call in flight", async () => {
    const signals: AbortSignal[] = [];
    vi.stubGlobal("fetch", vi.fn((_u: string, init: RequestInit) => { signals.push(init.signal!); return new Promise(() => {}); }));
    const { handler } = setup();
    void handler.call("run", ["s1", { input: "a" }]);
    void handler.call("run", ["s2", { input: "b" }]);
    await vi.waitFor(() => expect(signals).toHaveLength(2));
    handler.leave!();
    expect(signals.every(s => s.aborted)).toBe(true);
  });
});
