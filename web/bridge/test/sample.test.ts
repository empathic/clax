import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TEXT_INTERVAL_MS, sampleNamespace } from "../src/caps/sample";
import { CapabilityError } from "../src/rpc";

type Listener = (d: unknown) => void;
type Frame = { call: string; event: string; data: unknown };

function fakeRpc(run?: (call: string, req: Record<string, unknown>) => unknown) {
  const listeners = new Set<Listener>();
  const calls: { method: string; args: unknown[] }[] = [];
  const frame = (f: Frame) => { for (const l of [...listeners]) l(f); };
  return {
    calls,
    frame,
    rpc: {
      call: vi.fn(async (_ns: string, method: string, args: unknown[]) => {
        calls.push({ method, args });
        if (method === "run") return run ? run(args[0] as string, args[1] as Record<string, unknown>) : null;
        if (method === "limits") return { maxPromptBytes: 65536, tools: { maxCount: 16 } };
        return null;
      }),
      on: (_ns: string, _topic: string, f: Listener) => { listeners.add(f); return () => { listeners.delete(f); }; },
    },
  };
}

type SampleFn = ((input: unknown, options?: unknown) => Promise<unknown>) & { json(i: unknown, o?: unknown): Promise<unknown>; limits(): Promise<unknown> };

function setup(run?: (call: string, req: Record<string, unknown>) => unknown) {
  const f = fakeRpc(run);
  const sample = sampleNamespace(f.rpc as never) as unknown as SampleFn;
  const lastRun = () => f.calls.filter(c => c.method === "run").at(-1)!;
  return { ...f, sample, lastRun };
}

describe("sample", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("is a frozen function with json and limits", () => {
    const { sample } = setup();
    expect(typeof sample).toBe("function");
    expect(Object.isFrozen(sample)).toBe(true);
    expect(Object.keys(sample).sort()).toEqual(["json", "limits"]);
  });

  it("sends on the next microtask, streams whole text through onText, and resolves the result", async () => {
    const { sample, frame, calls, lastRun } = setup();
    const seen: { text: string; delta: string }[] = [];
    const p = sample("Summarize", { onText: (u: { text: string; delta: string }) => seen.push(u) });
    expect(calls).toEqual([]);
    await vi.advanceTimersByTimeAsync(0);
    const [call, req] = lastRun().args as [string, Record<string, unknown>];
    expect(req).toEqual({ input: "Summarize", verb: "text", model_tier: "default", tools: [], images: [], cache: true });
    frame({ call, event: "start", data: { call_id: "c", cached: false } });
    frame({ call, event: "text", data: { delta: " " } });
    frame({ call, event: "text", data: { delta: "Hel" } });
    expect(seen).toEqual([]);
    await vi.advanceTimersByTimeAsync(TEXT_INTERVAL_MS);
    frame({ call, event: "text", data: { delta: "lo" } });
    frame({ call: "someone-else", event: "text", data: { delta: "x" } });
    frame({ call, event: "done", data: { text: " Hello", truncated: false, model_tier_applied: "default" } });
    await expect(p).resolves.toEqual({ text: " Hello", truncated: false, modelTierApplied: "default" });
    expect(seen).toEqual([{ text: " Hel", delta: " Hel" }, { text: " Hello", delta: "lo" }]);
  });

  it("a cached answer is one onText call with the whole text", async () => {
    const { sample, frame, lastRun } = setup();
    const seen: unknown[] = [];
    const p = sample("q", { onText: (u: unknown) => seen.push(u) });
    await vi.advanceTimersByTimeAsync(0);
    const call = lastRun().args[0] as string;
    frame({ call, event: "start", data: { cached: true } });
    frame({ call, event: "done", data: { text: "whole", truncated: false, model_tier_applied: "quick" } });
    await p;
    expect(seen).toEqual([{ text: "whole", delta: "whole" }]);
  });

  it("an abort in the same block sends nothing; a later abort rejects cancelled with the kept text", async () => {
    const { sample, frame, calls, lastRun } = setup();
    const early = new AbortController();
    const p1 = sample("q", { signal: early.signal });
    early.abort();
    await expect(p1).rejects.toEqual({ code: "cancelled", message: expect.any(String) });
    expect(calls).toEqual([]);

    const ctl = new AbortController();
    const seen: string[] = [];
    const p2 = sample("q", { signal: ctl.signal, onText: ({ text }: { text: string }) => seen.push(text) });
    await vi.advanceTimersByTimeAsync(0);
    const call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "tick " } });
    await vi.advanceTimersByTimeAsync(TEXT_INTERVAL_MS);
    frame({ call, event: "text", data: { delta: "tick " } });
    ctl.abort();
    await expect(p2).rejects.toEqual({ code: "cancelled", message: expect.any(String), text: "tick " });
    expect(calls.at(-1)).toEqual({ method: "cancel", args: [call] });
    frame({ call, event: "text", data: { delta: "more" } });
    await vi.advanceTimersByTimeAsync(TEXT_INTERVAL_MS);
    expect(seen).toEqual(["tick "]);
  });

  it("a reused aborted signal rejects at once", async () => {
    const { sample, calls } = setup();
    const ctl = new AbortController();
    ctl.abort();
    await expect(sample("q", { signal: ctl.signal })).rejects.toMatchObject({ code: "cancelled" });
    expect(calls).toEqual([]);
  });

  it("errors carry the streamed text, except refused", async () => {
    const { sample, frame, lastRun } = setup();
    const p = sample("q");
    await vi.advanceTimersByTimeAsync(0);
    let call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "partial " } });
    frame({ call, event: "error", data: { code: "upstream_error", message: "broke" } });
    await expect(p).rejects.toEqual({ code: "upstream_error", message: "broke", text: "partial " });
    const r = sample("q");
    await vi.advanceTimersByTimeAsync(0);
    call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "I won" } });
    frame({ call, event: "error", data: { code: "refused", message: "no" } });
    await expect(r).rejects.toEqual({ code: "refused", message: "no" });
  });

  it("a refusal before the stream (consent, cap) rejects with that code", async () => {
    const { sample } = setup(() => { throw new CapabilityError("not_granted", "declined"); });
    const p = sample("q");
    const check = expect(p).rejects.toEqual({ code: "not_granted", message: "declined" });
    await vi.advanceTimersByTimeAsync(0);
    await check;
  });

  it("json resolves the parsed value and rejects invalid_json with the raw reply", async () => {
    const { sample, frame, lastRun } = setup();
    const p = sample.json("q");
    await vi.advanceTimersByTimeAsync(0);
    let call = lastRun().args[0] as string;
    expect((lastRun().args[1] as { verb: string }).verb).toBe("json");
    frame({ call, event: "done", data: { text: "[1]", truncated: false, model_tier_applied: "default", value: [1] } });
    await expect(p).resolves.toEqual([1]);
    const bad = sample.json("q");
    await vi.advanceTimersByTimeAsync(0);
    call = lastRun().args[0] as string;
    frame({ call, event: "text", data: { delta: "nope" } });
    frame({ call, event: "error", data: { code: "invalid_json", message: "no JSON" } });
    await expect(bad).rejects.toEqual({ code: "invalid_json", message: "no JSON", text: "nope" });
  });

  it("runs page tools and posts their results, errors included", async () => {
    const { sample, frame, calls, lastRun } = setup();
    const execute = vi.fn(async (input: { shade?: unknown }, _ctx?: { signal: AbortSignal }) => ({ color: `teal-${String(input.shade)}` }));
    const boom = vi.fn(() => { throw new Error("no such track"); });
    const p = sample("q", { tools: [
      { name: "getColor", description: "Returns the colour.", inputSchema: { type: "object", properties: { shade: { type: "string" } } }, execute },
      { name: "boom", description: "Fails.", execute: boom },
    ] });
    await vi.advanceTimersByTimeAsync(0);
    const [call, req] = lastRun().args as [string, { tools: unknown; cache: unknown }];
    expect(req.tools).toEqual([
      { name: "getColor", description: "Returns the colour.", input_schema: { type: "object", properties: { shade: { type: "string" } } } },
      { name: "boom", description: "Fails." },
    ]);
    expect(req.cache).toBe(false);
    frame({ call, event: "tool_call", data: { id: "t1", name: "getColor", input: { shade: "dark" } } });
    frame({ call, event: "tool_call", data: { id: "t2", name: "boom", input: {} } });
    frame({ call, event: "tool_call", data: { id: "t3", name: "missing", input: {} } });
    await vi.advanceTimersByTimeAsync(0);
    expect(execute.mock.calls[0][1]).toHaveProperty("signal");
    const results = calls.filter(c => c.method === "toolResult").map(c => c.args).sort((a, b) => String(a[1]).localeCompare(String(b[1])));
    expect(results).toEqual([
      [call, "t1", JSON.stringify({ color: "teal-dark" }), false],
      [call, "t2", "Error: no such track", true],
      [call, "t3", "Error: no tool named missing", true],
    ]);
    frame({ call, event: "done", data: { text: "ok", truncated: false, model_tier_applied: "default" } });
    await p;
  });

  it("a tool's signal aborts when the call settles", async () => {
    const { sample, frame, lastRun } = setup();
    let signal: AbortSignal | null = null;
    const p = sample("q", { tools: [{ name: "slow", description: "Waits.", execute: (_i: unknown, ctx: { signal: AbortSignal }) => { signal = ctx.signal; return new Promise(() => {}); } }] });
    await vi.advanceTimersByTimeAsync(0);
    const call = lastRun().args[0] as string;
    frame({ call, event: "tool_call", data: { id: "t1", name: "slow", input: {} } });
    await vi.advanceTimersByTimeAsync(0);
    expect(signal!.aborted).toBe(false);
    frame({ call, event: "error", data: { code: "upstream_error", message: "x" } });
    await p.catch(() => {});
    expect(signal!.aborted).toBe(true);
  });

  it("rejects malformed calls with invalid_request or prompt_too_large, sending nothing", async () => {
    const { sample, calls } = setup();
    const noop = () => 1;
    for (const [input, options] of [
      ["", undefined],
      [{ prompt: "q" }, undefined],
      [[], undefined],
      [[{ role: "assistant", content: "a" }], undefined],
      [[{ role: "user", content: "q" }, { role: "assistant", content: "a" }], undefined],
      [[{ role: "system", content: "q" }], undefined],
      ["q", "not an object"],
      ["q", new AbortController()],
      ["q", { signal: new AbortController() }],
      ["q", { onText: "x" }],
      ["q", { modelTier: "huge" }],
      ["q", { cache: "yes" }],
      ["q", { cache: { gcTime: 0 } }],
      ["q", { tools: "x" }],
      ["q", { tools: [{ name: "bad name", description: "d", execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "d", execute: noop }, { name: "a", description: "d", execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "", execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "d" }] }],
      ["q", { tools: [{ name: "a", description: "d", inputSchema: { type: "array" }, execute: noop }] }],
      ["q", { tools: [{ name: "a", description: "d", execute: noop }], cache: true }],
      ["q", { images: "not a blob" }],
    ] as [unknown, unknown][]) {
      await expect(sample(input, options), JSON.stringify([input, String(options)])).rejects.toMatchObject({ code: "invalid_request" });
    }
    await expect(sample("x".repeat(65_537))).rejects.toMatchObject({ code: "prompt_too_large" });
    expect(calls).toEqual([]);
  });

  it("maps cache options to the wire and ignores unknown option members", async () => {
    const { sample, lastRun } = setup();
    void sample("q", { cache: { gcTime: 1e12, refresh: true }, somethingNew: 1 });
    await vi.advanceTimersByTimeAsync(0);
    expect((lastRun().args[1] as { cache: unknown }).cache).toEqual({ gc_time_ms: 86_400_000, refresh: true });
    void sample("q", { cache: false, modelTier: "quick" });
    await vi.advanceTimersByTimeAsync(0);
    expect(lastRun().args[1]).toMatchObject({ cache: false, model_tier: "quick" });
  });

  it("limits comes from the shell", async () => {
    const { sample } = setup();
    await expect(sample.limits()).resolves.toEqual({ maxPromptBytes: 65536, tools: { maxCount: 16 } });
  });
});
