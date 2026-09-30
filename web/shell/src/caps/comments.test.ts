import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { forgetBudgets } from "./budget";
import { REFUSED_RATE, WRITE_RATE, cleanLabel, commentsHandler, textProblem } from "./comments";
import { forgetGestures, noteShellInput, notePointerOver } from "./gesture";
import { Grants } from "./grants";
import type { CapEnv, CommentsUi } from "./host";

const T = (id: string, anchor: Partial<Thread["anchor"]> = {}, more: Partial<Thread> = {}): Thread => ({
  id, artifact_id: "7q3k9mzx2b4t", version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null,
  created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "Alex", via_harness: null, body: "b", created_at: "x" }],
  anchor: { kind: "element", selector: "body > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html", ...anchor },
  ...more,
});

/** A viewer gesture in the content frame (activation, the pointer moved onto
 * `iframe.frame` and focus into it), or none: activation given to a shell
 * control under the pointer. */
const frame = document.createElement("iframe");
frame.className = "frame";
const control = document.createElement("button");
document.body.append(frame, control);
const gesture = (on: boolean) => {
  Object.defineProperty(navigator, "userActivation", { value: { isActive: true }, configurable: true });
  if (on) { control.focus(); notePointerOver(frame); frame.focus(); } else { notePointerOver(control); control.focus(); noteShellInput(); }
};

function setup(declared: Record<string, unknown>, answer: "allow" | "deny" | "dismiss" = "allow") {
  const posted: ShellToBridge[] = [];
  const state = { mode: false, composing: false, threads: [T("01J9A"), T("01J9B", { kind: "custom", selector: null, custom_name: "shape-1" }), T("01J9D", { file: "notes.html" })], selected: null as string | null, busy: false };
  const ui: Required<CommentsUi> = {
    openComposer: vi.fn(() => true), upsert: vi.fn(), remove: vi.fn(), setCustom: vi.fn(), place: vi.fn(), select: vi.fn(), exitMode: vi.fn(),
    state: () => state, dismiss: vi.fn(() => true), attachClip: vi.fn(), enterMode: vi.fn(() => { state.mode = true; }),
  };
  const prompt = vi.fn(async () => answer);
  const env = { aid: "7q3k9mzx2b4t", version: 1, token: "t", declared, prompt, post: (m: ShellToBridge) => posted.push(m), comments: ui, page: () => "index.html" } as unknown as CapEnv;
  const grants = new Grants("k", null, declared as never, true, prompt);
  return { h: commentsHandler(env, grants), ui, posted, prompt, state };
}

type Fetch = ReturnType<typeof vi.fn>;
/** A daemon answering every request with thread `t` (created or changed). */
function daemon(t: Thread = T("01J9C")) {
  const f = vi.fn(async (_url: string, _init?: RequestInit) => new Response(JSON.stringify({ thread: t, comment: { id: "cm" } }), { status: 201 }));
  vi.stubGlobal("fetch", f);
  return f;
}
const lastUrl = () => (fetch as unknown as Fetch).mock.calls.at(-1)![0] as string;
const threadsPushed = (posted: ShellToBridge[]) => posted.filter(m => m.type === "artifax:event" && m.topic === "threads") as unknown as { data: { list: { id: string; anchor: string }[] } }[];

describe("comments in the shell", () => {
  beforeEach(() => { gesture(true); });
  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
    forgetBudgets();
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
    forgetGestures();
  });

  it("text rule", () => {
    expect(textProblem("ok\n\tfine")).toBeNull();
    for (const bad of ["", "   ", "a\u0000", "é".repeat(2049), 5]) expect(textProblem(bad), String(bad)).not.toBeNull();
  });

  it("openComposer needs no consent, only the viewer's gesture, and is rate limited", async () => {
    const { h, ui, prompt } = setup({ comments: { composer_only: true } });
    const d = { anchor: T("x").anchor, version: 1 };
    gesture(false);
    expect(await h.call("openComposer", [d])).toEqual({ opened: false });
    gesture(true);
    for (let i = 0; i < 5; i++) expect(await h.call("openComposer", [d])).toEqual({ opened: true });
    await expect(h.call("openComposer", [d])).rejects.toMatchObject({ code: "rate_limited" });
    expect(prompt).not.toHaveBeenCalled();
    expect(ui.openComposer).toHaveBeenCalledTimes(5);
  });

  it("activation the viewer gave the shell is no gesture, and polling without one is cut off", async () => {
    const { h, ui, prompt } = setup({ comments: {} });
    const d = { anchor: T("x").anchor, version: 1 };
    // The viewer is typing in the shell (a reply, the consent dialog).
    gesture(false);
    expect(await h.call("openComposer", [d])).toEqual({ opened: false });
    // Focus in the frame, but the latest input went to the shell.
    frame.focus();
    noteShellInput();
    expect(await h.call("openComposer", [d])).toEqual({ opened: false });
    for (let i = 2; i < REFUSED_RATE.n; i++) await h.call("sendToClaude", [{ anchor: T("x").anchor, text: "x" }]).catch(() => {});
    await expect(h.call("openComposer", [d])).rejects.toMatchObject({ code: "rate_limited" });
    await expect(h.call("sendToClaude", [{ anchor: T("x").anchor, text: "x" }])).rejects.toMatchObject({ code: "rate_limited" });
    expect(ui.openComposer).not.toHaveBeenCalled();
    expect(prompt).not.toHaveBeenCalled();
    // A click in the frame is a gesture again.
    gesture(true);
    expect(await h.call("openComposer", [d])).toEqual({ opened: true });
  });

  it("write verbs ask once, post as written by the page, and answer opaque handles", async () => {
    const f = daemon();
    const { h, prompt, ui } = setup({ comments: {} });
    const r = (await h.call("create", [{ anchor: T("x").anchor, text: "hi @agent", version: 1 }])) as { threadId: string; commentId: string };
    expect(r.threadId).toMatch(/^t_[0-9a-f]{24}$/);
    expect(r.commentId).toMatch(/^c_[0-9a-f]{24}$/);
    const form = f.mock.calls[0][1]!.body as FormData;
    expect([form.get("via_page"), form.get("body")]).toEqual(["true", "hi @agent"]);
    const again = (await h.call("create", [{ anchor: T("x").anchor, text: "again", version: 1 }])) as { threadId: string };
    expect(again.threadId).not.toBe(r.threadId);
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(ui.upsert).toHaveBeenCalledTimes(2);
    expect(await h.call("reply", [r.threadId, "more"])).toMatchObject({ commentId: expect.stringMatching(/^c_/) });
    expect(lastUrl()).toBe("/api/artifacts/7q3k9mzx2b4t/threads/01J9C/comments");
    expect(JSON.parse(f.mock.calls.at(-1)![1]!.body as string)).toEqual({ body: "more", via_page: true });
    expect(await h.call("resolve", [r.threadId, false])).toBeUndefined();
    expect(lastUrl()).toBe("/api/artifacts/7q3k9mzx2b4t/threads/01J9C/reopen");
    expect(await h.call("delete", [r.threadId])).toBeUndefined();
    expect(f.mock.calls.at(-1)![1]).toMatchObject({ method: "DELETE" });
    expect(ui.remove).toHaveBeenCalledWith("01J9C");
    await expect(h.call("delete", [r.threadId])).rejects.toMatchObject({ code: "not_found" });
    expect(prompt).toHaveBeenCalledTimes(1);
    const only = setup({ comments: { composer_only: true } });
    await expect(only.h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }])).rejects.toMatchObject({ code: "not_granted" });
    expect(await only.h.call("canSendToClaude", [])).toBe("off");
  });

  it("write verbs act only on threads this page created in this document", async () => {
    const f = daemon();
    const { h, posted, state } = setup({ comments: { customAnchors: true } });
    await h.call("register", []);
    state.mode = true;
    h.uiChanged!();
    const pushed = threadsPushed(posted).at(-1)!.data.list[0].id;
    // Store IDs (what a page could glean from anywhere), list handles, and guesses.
    for (const id of ["01J9A", "01J9B", pushed, "t_000000000000000000000000"]) {
      await expect(h.call("reply", [id, "hi"]), id).rejects.toMatchObject({ code: "not_found" });
      await expect(h.call("resolve", [id, true]), id).rejects.toMatchObject({ code: "not_found" });
      await expect(h.call("delete", [id]), id).rejects.toMatchObject({ code: "not_found" });
      await expect(h.call("sendToClaude", [{ threadId: id, text: "hi" }]), id).rejects.toMatchObject({ code: "not_found" });
    }
    expect(f).not.toHaveBeenCalled();
    const { threadId } = (await h.call("create", [{ anchor: T("x").anchor, text: "mine" }])) as { threadId: string };
    await h.call("resolve", [threadId, true]);
    expect(lastUrl()).toBe("/api/artifacts/7q3k9mzx2b4t/threads/01J9C/resolve");
    // A new document in the frame starts with no threads of its own.
    h.reset!();
    const calls = f.mock.calls.length;
    await expect(h.call("delete", [threadId])).rejects.toMatchObject({ code: "not_found" });
    expect(f.mock.calls).toHaveLength(calls);
  });

  it("a reply the agent receives needs the viewer's gesture", async () => {
    const f = daemon(T("01J9C", {}, { sent_to_agent: true }));
    const { h, state } = setup({ comments: {} });
    const { threadId } = (await h.call("create", [{ anchor: T("x").anchor, text: "hi" }])) as { threadId: string };
    state.threads = [T("01J9C", {}, { sent_to_agent: true })];
    gesture(false);
    const calls = f.mock.calls.length;
    await expect(h.call("reply", [threadId, "more"])).rejects.toMatchObject({ code: "unavailable" });
    expect(f.mock.calls).toHaveLength(calls);
    state.threads = [T("01J9C")];
    expect(await h.call("reply", [threadId, "plain"])).toMatchObject({ commentId: expect.any(String) });
    state.threads = [T("01J9C", {}, { sent_to_agent: true })];
    gesture(true);
    expect(await h.call("reply", [threadId, "now"])).toMatchObject({ commentId: expect.any(String) });
  });

  it("a denial is forbidden and a dismissal consent_required, without asking again", async () => {
    const denied = setup({ comments: {} }, "deny");
    await expect(denied.h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }])).rejects.toMatchObject({ code: "forbidden" });
    await expect(denied.h.call("create", [{ anchor: T("x").anchor, text: "again", version: 1 }])).rejects.toMatchObject({ code: "forbidden" });
    expect(denied.prompt).toHaveBeenCalledTimes(1);
    const dismissed = setup({ comments: {} }, "dismiss");
    await expect(dismissed.h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }])).rejects.toMatchObject({ code: "consent_required" });
  });

  it("canSendToClaude is cached until an event may mean the sessions changed; sendToClaude needs a gesture and posts then sends", async () => {
    const urls: string[] = [];
    let live = false;
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      urls.push(url);
      if (url === "/api/artifacts/7q3k9mzx2b4t") return new Response(JSON.stringify({ artifact: { id: "7q3k9mzx2b4t", owner_live: live }, versions: [] }));
      return new Response(JSON.stringify({ thread: T("01J9C") }), { status: 201 });
    }));
    const { h } = setup({ comments: {} });
    const lookups = () => urls.filter(u => u === "/api/artifacts/7q3k9mzx2b4t").length;
    expect(await h.call("canSendToClaude", [])).toBe("no_session");
    live = true;
    expect(await h.call("canSendToClaude", [])).toBe("no_session");
    expect(lookups()).toBe(1);
    await expect(h.call("sendToClaude", [{ anchor: T("x").anchor, text: "please" }])).rejects.toMatchObject({ code: "claude_unavailable" });
    expect(urls.some(u => u.endsWith("/threads"))).toBe(false);
    h.onEvent!({ type: "feedback_state", artifact_id: "7q3k9mzx2b4t", thread_id: "x", state: "agent_ended", tier: null, since: "s", resends: 0, exhausted: false });
    expect(await h.call("canSendToClaude", [])).toBe("available");
    expect(lookups()).toBe(2);
    gesture(false);
    await expect(h.call("sendToClaude", [{ anchor: T("x").anchor, text: "please" }])).rejects.toMatchObject({ code: "claude_unavailable" });
    expect(urls.some(u => u.endsWith("/threads"))).toBe(false);
    gesture(true);
    const r = (await h.call("sendToClaude", [{ anchor: T("x").anchor, text: "please" }])) as { threadId: string };
    expect(r.threadId).toMatch(/^t_/);
    expect(urls.at(-1)).toBe("/api/artifacts/7q3k9mzx2b4t/threads/01J9C/send");
  });

  it("custom anchoring: register, thread handles, placement, reveal, release", async () => {
    const { h, ui, posted, state } = setup({ comments: { customAnchors: true } });
    await expect(setup({ comments: {} }).h.call("register", [])).rejects.toMatchObject({ code: "not_granted" });
    await h.call("register", []);
    expect(ui.setCustom).toHaveBeenCalledWith(true);
    state.mode = true;
    h.uiChanged!();
    const threads = threadsPushed(posted).at(-1)!;
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
    // Comment mode ends: the pins keep following the page's placements.
    state.mode = false;
    h.uiChanged!();
    await h.call("placed", [{ [handle]: { x: 5, y: 60 } }]);
    expect(ui.place).toHaveBeenLastCalledWith({ "01J9B": { x: 5, y: 60, w: 0, h: 0 } });
    await h.call("release", []);
    expect(ui.setCustom).toHaveBeenLastCalledWith(false);
    expect(h.reveal!("01J9B")).toBe(false);
  });

  it("compose is the viewer's click: it closes an open composer or card, starts the mode, and never stores the label", async () => {
    const { h, ui, posted, state } = setup({ comments: { customAnchors: true } });
    await h.call("register", []);
    gesture(false);
    expect(await h.call("compose", [{ anchor: "shape-2", dom: false, label: "Blue", version: 1 }])).toEqual({ opened: false });
    gesture(true);
    state.selected = "01J9A";
    expect(await h.call("compose", [{ anchor: "shape-2", dom: false, label: "Blue", version: 1 }])).toEqual({ opened: false });
    expect(ui.dismiss).toHaveBeenCalledTimes(1);
    expect(ui.openComposer).not.toHaveBeenCalled();
    state.selected = null;
    expect(await h.call("compose", [{ anchor: "shape-2", dom: false, label: "  Blue\n square ", version: 1 }])).toEqual({ opened: true });
    expect(ui.enterMode).toHaveBeenCalledTimes(1);
    const opened = (ui.openComposer as Fetch).mock.calls.at(-1)![0];
    expect(opened.anchor).toMatchObject({ kind: "custom", custom_name: "shape-2", quote: null, selector: null, file: "index.html" });
    expect(opened.label).toBe("Blue square");
    // A session the page started gets the mode, but no thread list.
    h.uiChanged!();
    expect(posted.filter(m => m.type === "artifax:event" && m.topic === "mode").at(-1)).toMatchObject({ data: { on: true } });
    expect(threadsPushed(posted)).toHaveLength(0);
    state.composing = false;
    expect(await h.call("compose", [{ anchor: "body > h2", dom: true, label: "Title", version: 1 }])).toEqual({ opened: true });
    expect((ui.openComposer as Fetch).mock.calls.at(-1)![0].anchor).toMatchObject({ kind: "element", selector: "body > h2", quote: null });
  });

  it("compose with an area is gesture-checked before any clip, anchors the element, and takes its clip once by nonce", async () => {
    const { h, ui, state, posted } = setup({ comments: { customAnchors: true } });
    await h.call("register", []);
    state.mode = true;
    const areaCall = () => h.call("compose", [{ anchor: "body > main > h2", dom: true, area: true, clipPending: true, version: 1 }]) as Promise<{ opened: boolean; clipNonce?: string }>;
    gesture(false);
    expect(await areaCall()).toEqual({ opened: false });
    expect(ui.openComposer).not.toHaveBeenCalled();
    gesture(true);
    // Over an open composer or card an area opens (or moves) the composer instead of dismissing.
    state.composing = true;
    state.selected = "01J9A";
    const r = await areaCall();
    expect(r.opened).toBe(true);
    expect(r.clipNonce).toMatch(/^[0-9a-f]{24}$/);
    expect(ui.dismiss).not.toHaveBeenCalled();
    const [d, opts] = (ui.openComposer as Fetch).mock.calls.at(-1)!;
    // The page's geometry is unknown: the element, not an area, is stored.
    expect(d.anchor).toMatchObject({ kind: "element", selector: "body > main > h2", quote: null, file: "index.html" });
    expect(d.anchor.area).toBeUndefined();
    expect(d).toMatchObject({ clip: null, capturing: true, clipToken: r.clipNonce });
    expect(opts).toEqual({ area: true });
    // The clip arrives once under the nonce; a replay or an unknown nonce does nothing.
    await h.call("composeClip", [{ nonce: r.clipNonce, clipPng: new Uint8Array([137, 80, 78, 71]).buffer }]);
    expect(ui.attachClip).toHaveBeenCalledTimes(1);
    const [token, blob, err] = (ui.attachClip as Fetch).mock.calls[0];
    expect(token).toBe(r.clipNonce);
    expect(blob).toBeInstanceOf(Blob);
    expect(err).toBeUndefined();
    await h.call("composeClip", [{ nonce: r.clipNonce, clipPng: new Uint8Array([1]).buffer }]);
    await h.call("composeClip", [{ nonce: "forged", clipPng: new Uint8Array([1]).buffer }]);
    expect(ui.attachClip).toHaveBeenCalledTimes(1);
    // An oversized clip is refused with openComposer's wording.
    const r2 = await areaCall();
    await h.call("composeClip", [{ nonce: r2.clipNonce, clipPng: new ArrayBuffer(5 * 1024 * 1024 + 1) }]);
    expect((ui.attachClip as Fetch).mock.calls.at(-1)!.slice(1)).toEqual([null, "the screenshot was too large"]);
    // While a post or send is in flight the page is told areas are off, and the flag is ignored.
    state.busy = true;
    h.uiChanged!();
    expect(posted.filter(m => m.type === "artifax:event" && m.topic === "mode").at(-1)).toMatchObject({ data: { on: true, canArea: false } });
    expect(await areaCall()).toEqual({ opened: false });
    expect(ui.dismiss).toHaveBeenCalledTimes(1);
    state.busy = false;
    h.uiChanged!();
    expect(posted.filter(m => m.type === "artifax:event" && m.topic === "mode").at(-1)).toMatchObject({ data: { on: true, canArea: true } });
    // Outside comment mode the flag is ignored: the usual element anchor, no nonce.
    state.mode = false;
    state.composing = false;
    state.selected = null;
    const r3 = await areaCall();
    expect(r3.clipNonce).toBeUndefined();
    expect((ui.openComposer as Fetch).mock.calls.at(-1)![0]).toMatchObject({ anchor: { kind: "element" }, clipError: "anchored by the page" });
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
      ["reply", [{ id: "01J9A" }, "hi"]],
      ["resolve", [5, true]],
      ["delete", [null]],
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
    const f = daemon();
    const { h, ui } = setup({ comments: {} });
    await h.call("create", [{ anchor: { ...T("x").anchor, file: "../../etc/passwd", extra: "x" }, text: "hi", version: 9 }]);
    const form = f.mock.calls[0][1]!.body as FormData;
    expect(JSON.parse(form.get("anchor") as string)).toEqual({ ...T("x").anchor, file: "index.html" });
    expect(form.get("version")).toBe("1");
    await h.call("openComposer", [{ anchor: { ...T("x").anchor, file: "x.html" }, version: 7 }]);
    expect((ui.openComposer as Fetch).mock.calls[0][0]).toMatchObject({ anchor: { file: "index.html" }, version: 1 });
  });

  it("page writes are budgeted per tab", async () => {
    const f = daemon();
    const { h } = setup({ comments: {} });
    const { threadId } = (await h.call("create", [{ anchor: T("x").anchor, text: "hi" }])) as { threadId: string };
    for (let i = 1; i < WRITE_RATE.n; i++) await h.call("resolve", [threadId, true]);
    await expect(h.call("create", [{ anchor: T("x").anchor, text: "hi" }])).rejects.toMatchObject({ code: "rate_limited" });
    expect(f.mock.calls).toHaveLength(WRITE_RATE.n);
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

  it("labels follow the contract's rule", () => {
    expect(cleanLabel("  Red\n\tsquare​ ")).toBe("Red square");
    expect(cleanLabel("---")).toBeNull();
    expect(new TextEncoder().encode(cleanLabel("é".repeat(100))!).length).toBeLessThanOrEqual(128);
  });
});
