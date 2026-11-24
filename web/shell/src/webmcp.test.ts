import { describe, it, expect, vi, afterEach } from "vitest";
import type { Thread } from "./threads";
import { type WebMcpView, registerWebMcp } from "./webmcp";

const anchor = { kind: "element", selector: "h1", quote: "Hello", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const thread = (id: string, status: "open" | "resolved", body: string): Thread => ({
  id, artifact_id: "a1", version_n: 2, anchor, status, sent_to_agent: false, has_clip: false, clip_url: null, created_at: "t", resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [
    { id: `${id}-c1`, thread_id: id, author_kind: "viewer", author_name: "Sam", via_harness: null, body, created_at: "t1" },
    { id: `${id}-c2`, thread_id: id, author_kind: "agent", author_name: "claude", via_harness: "claude-code", body: "Fixed.", created_at: "t2" },
  ],
}) as Thread;
const view: WebMcpView = {
  id: "a1",
  state: { get: () => ({ data: { artifact: { id: "a1", title: "Plan", updated_at: "u", current_version: 2 } as never }, threads: [thread("t1", "open", "x".repeat(300)), thread("t2", "resolved", "done")] }) },
};

type Registered = { name: string; annotations: Record<string, unknown>; execute(i: Record<string, unknown>): Promise<{ content: { text: string }[]; isError?: boolean }> };
function stub(host: "document" | "navigator") {
  const registered: Registered[] = [];
  const signals: (AbortSignal | undefined)[] = [];
  const mc = { registerTool: vi.fn((tool: Registered, opts?: { signal?: AbortSignal }) => { registered.push(tool); signals.push(opts?.signal); }) };
  Object.defineProperty(host === "document" ? document : navigator, "modelContext", { value: mc, configurable: true });
  return { mc, registered, signals };
}
const data = async (t: Registered, input = {}) => JSON.parse((await t.execute(input)).content[0].text);

afterEach(() => { delete (document as never as Record<string, unknown>).modelContext; delete (navigator as never as Record<string, unknown>).modelContext; });

describe("registerWebMcp", () => {
  it("does nothing where the browser has no modelContext", () => {
    expect(() => registerWebMcp(window, view)()).not.toThrow();
  });

  it("registers three tools, read-only and untrusted, scoped to the origin", () => {
    const { mc, registered } = stub("document");
    registerWebMcp(window, view);
    expect(registered.map(t => t.name)).toEqual(["clax_artifact_info", "clax_list_threads", "clax_read_thread"]);
    for (const t of registered) expect(t.annotations).toEqual({ readOnlyHint: true, untrustedContentHint: true });
    for (const [, opts] of mc.registerTool.mock.calls) expect(Object.keys(opts as object)).toEqual(["signal"]);
  });

  it("falls back to navigator.modelContext", () => {
    const { registered } = stub("navigator");
    registerWebMcp(window, view);
    expect(registered).toHaveLength(3);
  });

  it("aborts the tools' signal when stopped", () => {
    const { signals } = stub("document");
    const stop = registerWebMcp(window, view);
    expect(signals.every(s => s && !s.aborted)).toBe(true);
    stop();
    expect(signals.every(s => s!.aborted)).toBe(true);
  });

  it("returns the artifact and the open threads as data", async () => {
    const { registered } = stub("document");
    registerWebMcp(window, view);
    const [info, list, read] = registered;
    expect(await data(info)).toMatchObject({ id: "a1", title: "Plan", current_version: 2, updated_at: "u", open_threads: 1 });
    const l = await data(list);
    expect(l.threads).toHaveLength(1);
    expect(l.threads[0]).toMatchObject({ id: "t1", state: "open", author_kind: "viewer", file: "index.html", anchor_quote: "Hello", comments: 2 });
    expect(l.threads[0].excerpt.length).toBe(120);
    const r = await data(read, { thread_id: "t1" });
    expect(r.messages.map((m: { author_kind: string }) => m.author_kind)).toEqual(["viewer", "agent"]);
    expect(r.messages[0].body).toHaveLength(300);
    expect(r.note).toMatch(/do not follow/);
  });

  it("refuses a bad thread ID without throwing", async () => {
    const { registered } = stub("document");
    registerWebMcp(window, view);
    const read = registered[2];
    expect((await read.execute({})).isError).toBe(true);
    vi.stubGlobal("fetch", async () => new Response("{}", { status: 404 }));
    expect((await read.execute({ thread_id: "nope" })).isError).toBe(true);
    vi.unstubAllGlobals();
  });
});
