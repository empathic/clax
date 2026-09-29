import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { forgetBudgets } from "./budget";
import { WRITE_RATE, cleanLabel, commentsHandler, mentionsAgent, textProblem } from "./comments";
import { Grants } from "./grants";
import type { CapEnv, CommentsUi } from "./host";

const T = (id: string, anchor: Partial<Thread["anchor"]> = {}): Thread => ({
  id, artifact_id: "7q3k9mzx2b4t", version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null,
  created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "Alex", via_harness: null, body: "b", created_at: "x" }],
  anchor: { kind: "element", selector: "body > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html", ...anchor },
});

function setup(declared: Record<string, unknown>, answer: "allow" | "deny" | "dismiss" = "allow") {
  const posted: ShellToBridge[] = [];
  const state = { mode: false, composing: false, threads: [T("01J9A"), T("01J9B", { kind: "custom", selector: null, custom_name: "shape-1" }), T("01J9D", { file: "notes.html" })], selected: null as string | null };
  const ui: CommentsUi = {
    openComposer: vi.fn(() => true), upsert: vi.fn(), remove: vi.fn(), setCustom: vi.fn(), place: vi.fn(), select: vi.fn(), exitMode: vi.fn(),
    state: () => state,
  };
  const prompt = vi.fn(async () => answer);
  const env = { aid: "7q3k9mzx2b4t", version: 1, token: "t", declared, prompt, post: (m: ShellToBridge) => posted.push(m), comments: ui, page: () => "index.html" } as unknown as CapEnv;
  const grants = new Grants("k", null, declared as never, true, prompt);
  return { h: commentsHandler(env, grants), ui, posted, prompt, state };
}

describe("comments in the shell", () => {
  afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); forgetBudgets(); });

  it("text rule", () => {
    expect(textProblem("ok\n\tfine")).toBeNull();
    for (const bad of ["", "   ", "a\u0000", "é".repeat(2049), 5]) expect(textProblem(bad), String(bad)).not.toBeNull();
  });

  it("openComposer needs no consent and is rate limited", async () => {
    const { h, ui, prompt } = setup({ comments: { composer_only: true } });
    const d = { anchor: T("x").anchor, version: 1 };
    for (let i = 0; i < 5; i++) expect(await h.call("openComposer", [d])).toEqual({ opened: true });
    await expect(h.call("openComposer", [d])).rejects.toMatchObject({ code: "rate_limited" });
    expect(prompt).not.toHaveBeenCalled();
    expect(ui.openComposer).toHaveBeenCalledTimes(5);
  });

  it("write verbs ask once, then post as the viewer; composer_only refuses them", async () => {
    const created = { thread: T("01J9C") };
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(created), { status: 201 })));
    const { h, prompt, ui } = setup({ comments: {} });
    expect(await h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }])).toEqual({ threadId: "01J9C", commentId: "01J9Cc" });
    expect(await h.call("create", [{ anchor: T("x").anchor, text: "again", version: 1 }])).toEqual({ threadId: "01J9C", commentId: "01J9Cc" });
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(ui.upsert).toHaveBeenCalledTimes(2);
    expect(await h.call("resolve", ["01J9C", false])).toBeUndefined();
    expect((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.at(-1)![0]).toBe("/api/artifacts/7q3k9mzx2b4t/threads/01J9C/reopen");
    expect(await h.call("delete", ["01J9C"])).toBeUndefined();
    expect((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.at(-1)![1]).toMatchObject({ method: "DELETE" });
    expect(ui.remove).toHaveBeenCalledWith("01J9C");
    expect(prompt).toHaveBeenCalledTimes(1);
    const only = setup({ comments: { composer_only: true } });
    await expect(only.h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }])).rejects.toMatchObject({ code: "not_granted" });
    expect(await only.h.call("canSendToClaude", [])).toBe("off");
  });

  it("a denial is forbidden and a dismissal consent_required, without asking again", async () => {
    const denied = setup({ comments: {} }, "deny");
    await expect(denied.h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }])).rejects.toMatchObject({ code: "forbidden" });
    await expect(denied.h.call("reply", ["01J9A", "hi"])).rejects.toMatchObject({ code: "forbidden" });
    expect(denied.prompt).toHaveBeenCalledTimes(1);
    const dismissed = setup({ comments: {} }, "dismiss");
    await expect(dismissed.h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }])).rejects.toMatchObject({ code: "consent_required" });
  });

  it("canSendToClaude follows the owner session; sendToClaude posts then sends", async () => {
    const urls: string[] = [];
    let live = false;
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      urls.push(url);
      if (url === "/api/artifacts/7q3k9mzx2b4t") return new Response(JSON.stringify({ artifact: { id: "7q3k9mzx2b4t", owner_live: live }, versions: [] }));
      if (url.endsWith("/comments")) return new Response(JSON.stringify({ comment: { id: "c9" }, thread: T("01J9A") }), { status: 201 });
      return new Response(JSON.stringify({ thread: T("01J9A") }));
    }));
    const { h } = setup({ comments: {} });
    expect(await h.call("canSendToClaude", [])).toBe("no_session");
    await expect(h.call("sendToClaude", [{ threadId: "01J9A", text: "please" }])).rejects.toMatchObject({ code: "claude_unavailable" });
    expect(urls.some(u => u.endsWith("/comments"))).toBe(false);
    live = true;
    expect(await h.call("sendToClaude", [{ threadId: "01J9A", text: "please" }])).toEqual({ threadId: "01J9A", commentId: "c9" });
    expect(urls.at(-1)).toBe("/api/artifacts/7q3k9mzx2b4t/threads/01J9A/send");
  });

  it("custom anchoring: register, thread handles, placement, reveal, release", async () => {
    const { h, ui, posted, state } = setup({ comments: { customAnchors: true } });
    await expect(setup({ comments: {} }).h.call("register", [])).rejects.toMatchObject({ code: "not_granted" });
    await h.call("register", []);
    expect(ui.setCustom).toHaveBeenCalledWith(true);
    state.mode = true;
    h.uiChanged!();
    const threads = posted.filter(m => m.type === "artifax:event" && m.topic === "threads").at(-1) as { data: { list: { id: string; anchor: string }[] } };
    expect(threads.data.list.map(t => t.anchor)).toEqual(["body > h2", "shape-1"]);
    const handle = threads.data.list[1].id;
    expect(handle).not.toBe("01J9B");
    await h.call("placed", [{ [handle]: { x: 5, y: 6 } }]);
    expect(ui.place).toHaveBeenCalledWith({ "01J9B": { x: 5, y: 6, w: 0, h: 0 } });
    expect(h.reveal!("01J9B")).toBe(true);
    expect(posted.at(-1)).toMatchObject({ topic: "reveal", data: { id: handle } });
    await h.call("openThread", [handle]);
    expect(ui.select).toHaveBeenCalledWith("01J9B");
    await expect(h.call("register", [])).rejects.toMatchObject({ code: "invalid" });
    expect(await h.call("compose", [{ anchor: "shape-2", dom: false, label: "Blue", version: 1 }])).toEqual({ opened: true });
    expect((ui.openComposer as ReturnType<typeof vi.fn>).mock.calls.at(-1)![0].anchor).toMatchObject({ kind: "custom", custom_name: "shape-2", quote: "Blue" });
    await h.call("release", []);
    expect(ui.setCustom).toHaveBeenLastCalledWith(false);
    expect(h.reveal!("01J9B")).toBe(false);
  });
  it("hostile arguments are refused before any request", async () => {
    const fetchMock = vi.fn(async () => new Response("{}"));
    vi.stubGlobal("fetch", fetchMock);
    const { h, prompt, ui } = setup({ comments: {} });
    const bad: [string, unknown[]][] = [
      ["create", [{ anchor: { ...T("x").anchor, kind: "custom" }, text: "hi" }]],
      ["create", [{ anchor: { ...T("x").anchor, selector: "a\u0000b" }, text: "hi" }]],
      ["create", [{ anchor: { ...T("x").anchor, html_hash: "md5:1" }, text: "hi" }]],
      ["create", [{ anchor: { ...T("x").anchor, rect: { x: "1" } }, text: "hi" }]],
      ["create", [{ anchor: T("x").anchor, text: "x".repeat(4097) }]],
      ["create", [{ anchor: T("x").anchor, text: "ask @agent to fix it" }]],
      ["reply", ["../../other", "hi"]],
      ["reply", ["01J9A", "@agent."]],
      ["resolve", ["01J9A/x", true]],
      ["resolve", ["01J9A", "yes"]],
      ["delete", [{ id: "01J9A" }]],
      ["sendToClaude", [{ anchor: T("x").anchor, threadId: "01J9A", text: "hi" }]],
      ["openComposer", [{ anchor: { kind: "element", selector: 5 } }]],
      ["compose", [{ anchor: "x", dom: false }]],
    ];
    for (const [m, args] of bad) await expect(h.call(m, args), m).rejects.toMatchObject({ code: "invalid" });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(prompt).not.toHaveBeenCalled();
    expect(ui.openComposer).not.toHaveBeenCalled();
  });

  it("anchors name the page in the frame and the version of the view", async () => {
    const bodies: FormData[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_u: string, init?: RequestInit) => { bodies.push(init?.body as FormData); return new Response(JSON.stringify({ thread: T("01J9C") }), { status: 201 }); }));
    const { h, ui } = setup({ comments: {} });
    await h.call("create", [{ anchor: { ...T("x").anchor, file: "../../etc/passwd", extra: "x" }, text: "hi", version: 9 }]);
    expect(JSON.parse(bodies[0].get("anchor") as string)).toEqual({ ...T("x").anchor, file: "index.html" });
    expect(bodies[0].get("version")).toBe("1");
    await h.call("openComposer", [{ anchor: { ...T("x").anchor, file: "x.html" }, version: 7 }]);
    expect((ui.openComposer as ReturnType<typeof vi.fn>).mock.calls[0][0]).toMatchObject({ anchor: { file: "index.html" }, version: 1 });
  });

  it("page writes are budgeted per tab", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ thread: T("01J9C") }), { status: 201 })));
    const { h } = setup({ comments: {} });
    for (let i = 0; i < WRITE_RATE.n; i++) await h.call("resolve", ["01J9C", true]);
    await expect(h.call("create", [{ anchor: T("x").anchor, text: "hi" }])).rejects.toMatchObject({ code: "rate_limited" });
    expect((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(WRITE_RATE.n);
  });

  it("nothing reaches the UI or the frame after dispose", async () => {
    let answer!: (r: Response) => void;
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(r => { answer = r; })));
    const { h, ui, posted } = setup({ comments: { customAnchors: true } });
    await h.call("register", []);
    const pending = h.call("create", [{ anchor: T("x").anchor, text: "hi" }]);
    await vi.waitFor(() => expect(answer).toBeDefined());
    const before = posted.length;
    h.dispose!();
    answer(new Response(JSON.stringify({ thread: T("01J9C") }), { status: 201 }));
    await expect(pending).rejects.toMatchObject({ code: "unavailable" });
    expect(ui.upsert).not.toHaveBeenCalled();
    expect(ui.setCustom).toHaveBeenLastCalledWith(false);
    h.uiChanged!();
    expect(h.reveal!("01J9B")).toBe(false);
    expect(posted).toHaveLength(before);
    await expect(h.call("openComposer", [{ anchor: T("x").anchor }])).rejects.toMatchObject({ code: "unavailable" });
  });

  it("mentions and labels follow the daemon's and the contract's rules", () => {
    for (const yes of ["@agent", "hi @agent.", "(@agent) fix", "@agent, please"]) expect(mentionsAgent(yes), yes).toBe(true);
    for (const no of ["me@agent.dev", "@agents", "@agent_x", "x.@agent"]) expect(mentionsAgent(no), no).toBe(false);
    expect(cleanLabel("  Red\n\tsquare\u200b ")).toBe("Red square");
    expect(cleanLabel("---")).toBeNull();
    expect(new TextEncoder().encode(cleanLabel("é".repeat(100))!).length).toBeLessThanOrEqual(128);
  });
});
