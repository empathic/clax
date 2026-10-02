import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import type { Thread } from "../threads";

const ID = "7q3k9mzx2b4t";
const loaded = { artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "x", current_version: 2, pinned: false }, versions: [{ artifact_id: ID, n: 2, label: null, created_at: "x", files: {} }] };

class FakeES {
  static last: FakeES | undefined;
  listeners = new Map<string, (e: MessageEvent) => void>();
  constructor() { FakeES.last = this; }
  addEventListener(t: string, fn: (e: MessageEvent) => void) { this.listeners.set(t, fn); }
  close() {}
  emit(t: string, data: unknown) { this.listeners.get(t)?.(new MessageEvent(t, { data: JSON.stringify(data) })); }
}

type Seed = { threads?: Thread[]; artifact?: Record<string, unknown>; versions?: unknown[]; attention?: unknown; routes?: (url: string, init?: RequestInit) => unknown };
const thread = (id: string, over: Partial<Thread> = {}): Thread => ({
  id, artifact_id: ID, version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "2026-09-30T10:00:00.000Z",
  resolved_at: null, resolved_by: null, feedback_state: null, comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "alex", via_harness: null, body: "x", created_at: "2026-09-30T10:00:00.000Z" }],
  anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }, ...over,
});

async function started(seed: Seed = {}) {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const own = seed.routes?.(url, init);
    if (own !== undefined) return new Response(JSON.stringify(own));
    return new Response(JSON.stringify(
      url.includes("/threads") ? { threads: seed.threads ?? [], next_cursor: null }
      : url.startsWith("/api/viewers") ? { viewer: { public_id: "u_1", display_name: null, created_at: "x" } }
      : url === "/api/token" ? { token: "tk" }
      : { ...loaded, artifact: { ...loaded.artifact, ...seed.artifact }, versions: seed.versions ?? loaded.versions, ...(seed.attention ? { attention: seed.attention } : {}) }));
  }));
  sessionStorage.setItem("clax.origin-ok", "0");
  const { ArtifactController } = await import("./artifact-controller");
  const { FrameHost } = await import("./frame-host");
  const stage = document.createElement("div");
  document.body.append(stage);
  const ctl = new ArtifactController({ id: ID, pinnedVersion: null });
  ctl.frame = new FrameHost(stage, () => ctl.frameLoaded());
  ctl.start();
  const deadline = Date.now() + 2000;
  while (!stage.querySelector("iframe") && Date.now() < deadline) await new Promise(r => setTimeout(r, 5));
  return { ctl, frame: stage.querySelector("iframe")! };
}

const fromFrame = (win: Window, data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: "null", source: win }));
const hello = (win: Window, version = 2) => fromFrame(win, { type: "clax:hello", artifact: ID, version, file: "index.html" });

const seed: Seed = {
  threads: [thread("t1")],
  versions: [
    { artifact_id: ID, n: 1, label: null, created_at: "2026-09-30T09:00:00.000Z", files: {} },
    { artifact_id: ID, n: 2, label: null, created_at: "2026-09-30T11:00:00.000Z", files: {}, addresses: ["t1"], note: "Two columns", agent: "a_1", agent_harness: "claude" },
  ],
  attention: { addressed: ["t1"], addressed_v: 2, new_replies: [], open_in: ["t1"], seen: 1, looked: {} },
  routes: url => (url === "/api/viewers/me/seen" ? { seen: 2 } : url === "/api/viewers/me/looked" ? { looked: { t1: "x" } } : undefined),
};

describe("ArtifactController", () => {
  beforeEach(() => { vi.resetModules(); FakeES.last = undefined; history.replaceState(null, "", `/a/${ID}`); });
  // The gesture module this test's registry loaded watches the document until its `unwatchShell` runs.
  afterEach(async () => { (await import("../caps/gesture")).unwatchShell(); vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); });

  it("decides the changelog once ready, writes seen for the unpinned latest, and batches looked-at marks", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { ctl } = await started(seed);
    await vi.waitFor(() => expect(ctl.state.get().decided).toEqual({ n: 2, ids: ["t1"], dot: true, line: "v2 addressed 1" }));
    const puts = () => (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.filter(([, i]) => (i as RequestInit | undefined)?.method === "PUT");
    await vi.waitFor(() => expect(puts().some(([u, i]) => String(u) === "/api/viewers/me/seen" && JSON.parse((i as RequestInit).body as string).version === 2)).toBe(true));
    const t1 = ctl.state.get().threads.find(t => t.id === "t1")!;
    ctl.look(t1);
    ctl.look(t1);
    await vi.advanceTimersByTimeAsync(1100);
    const looked = puts().filter(([u]) => String(u) === "/api/viewers/me/looked");
    expect(looked).toHaveLength(1);
    expect(JSON.parse((looked[0][1] as RequestInit).body as string)).toEqual({ artifact_id: ID, thread_ids: ["t1"] });
    expect(ctl.state.get().decided!.ids).toEqual(["t1"]);
    ctl.dispose();
    vi.useRealTimers();
  });

  it("inserts the frame only once the artifact and the frame mode are known, with the gate already reset", async () => {
    const { ctl, frame } = await started();
    expect(ctl.state.get().data?.artifact.title).toBe("T");
    expect(ctl.state.get().origin).toBeNull();
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    const posted: { type: string }[] = [];
    frame.contentWindow!.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    // A hello for another version of the artifact is not welcomed.
    hello(frame.contentWindow!, 1);
    expect(posted.map(m => m.type)).not.toContain("clax:welcome");
    hello(frame.contentWindow!);
    expect(posted.map(m => m.type)).toContain("clax:welcome");
    ctl.dispose();
  });

  it("says when a lazy part of the bridge could not load, only for the greeted page and a known part", async () => {
    const { ctl, frame } = await started();
    frame.contentWindow!.postMessage = (() => {}) as Window["postMessage"];
    const degraded = (part: unknown, message: unknown = "blocked by CSP") => fromFrame(frame.contentWindow!, { type: "clax:degraded", part, message });
    // Before the page greeted, nothing is said.
    degraded("comment");
    expect(ctl.state.get().notice).toBeNull();
    hello(frame.contentWindow!);
    ctl.toggleComment();
    expect(ctl.state.get().commenting).toBe(true);
    for (const part of ["toString", "__proto__", "nope", 7]) degraded(part);
    expect(ctl.state.get().notice).toBeNull();
    // The page's own words never reach the shell's notice.
    degraded("clip", "Your session expired: sign in at evil.example");
    expect(ctl.state.get().notice).toBe("Screenshots could not load in this page.");
    expect(ctl.state.get().commenting).toBe(true);
    degraded("comment");
    expect(ctl.state.get().notice).toBe("Comment mode could not load in this page.");
    expect(ctl.state.get().commenting).toBe(false);
    // The comment notice outranks the clip notice.
    degraded("clip");
    expect(ctl.state.get().notice).toBe("Comment mode could not load in this page.");
    // While the comment part has failed, pressing Comment keeps comment mode
    // off and says so again, even after the notice was dismissed.
    ctl.dismissNotice();
    ctl.toggleComment();
    expect(ctl.state.get().commenting).toBe(false);
    expect(ctl.state.get().notice).toBe("Comment mode could not load in this page.");
    // The page's next greeting forgets it.
    hello(frame.contentWindow!);
    ctl.dismissNotice();
    ctl.toggleComment();
    expect(ctl.state.get().commenting).toBe(true);
    expect(ctl.state.get().notice).toBeNull();
    ctl.dispose();
  });

  it("forgets a failed part when the frame's document goes without greeting, not at its own load", async () => {
    const { ctl, frame } = await started();
    frame.contentWindow!.postMessage = (() => {}) as Window["postMessage"];
    hello(frame.contentWindow!);
    fromFrame(frame.contentWindow!, { type: "clax:degraded", part: "comment", message: "x" });
    // The greeted document's own load keeps what it reported.
    ctl.frameLoaded();
    ctl.dismissNotice();
    ctl.toggleComment();
    expect(ctl.state.get().commenting).toBe(false);
    expect(ctl.state.get().notice).toBe("Comment mode could not load in this page.");
    // A document that loaded without greeting: the gate closed, and the
    // failure belonged to the page before it.
    ctl.frameLoaded();
    ctl.dismissNotice();
    ctl.toggleComment();
    expect(ctl.state.get().commenting).toBe(true);
    expect(ctl.state.get().notice).toBeNull();
    ctl.dispose();
  });

  it("tells the capability host about UI changes once per pass after the paint, however many fields changed", async () => {
    const { ctl, frame } = await started();
    hello(frame.contentWindow!);
    const { CapabilityHost } = await import("../caps/host");
    const ui = vi.spyOn(CapabilityHost.prototype, "uiChanged");
    ctl.toggleComment();
    ctl.togglePanel();
    ctl.hover(null);
    ctl.toggleComment();
    ctl.toggleComment();
    await Promise.resolve();
    expect(ui).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(ui).toHaveBeenCalledTimes(1));
    await new Promise(r => setTimeout(r, 150));
    expect(ui).toHaveBeenCalledTimes(1);
    ctl.dispose();
  });

  it("tells a capability host made by an update of the UI only in the pass after the paint", async () => {
    const { ctl } = await started();
    await new Promise(r => setTimeout(r, 150));
    const { CapabilityHost } = await import("../caps/host");
    const told: unknown[] = [];
    vi.spyOn(CapabilityHost.prototype, "uiChanged").mockImplementation(function (this: unknown) { told.push(this); });
    ctl.update({ id: ID, pinnedVersion: 1 });
    await Promise.resolve();
    await Promise.resolve();
    expect(told).toEqual([]);
    await vi.waitFor(() => expect(told).toHaveLength(1));
    await new Promise(r => setTimeout(r, 150));
    expect(told).toHaveLength(1);
    // The host the update made, not the one it replaced.
    expect((told[0] as { dead: boolean }).dead).toBe(false);
    ctl.dispose();
  });

  it("after dispose, answers no hello and changes no state", async () => {
    const { ctl, frame } = await started();
    const posted: unknown[] = [];
    frame.contentWindow!.postMessage = ((m: unknown) => { posted.push(m); }) as Window["postMessage"];
    ctl.dispose();
    hello(frame.contentWindow!);
    const before = ctl.state.get();
    ctl.toggleComment();
    expect(posted).toEqual([]);
    expect(ctl.state.get()).toBe(before);
  });

  it("in sandbox mode, posts nothing to a document that has not greeted, and sends it all again at its hello", async () => {
    const { ctl, frame } = await started();
    const win = frame.contentWindow!;
    const posted: { type: string; anchors?: { id: string }[] }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    hello(win);
    frame.dispatchEvent(new Event("load"));
    expect(posted.map(m => m.type)).toContain("clax:welcome");
    // The frame moves to a document without the bridge (a foreign site): its
    // load, with no hello since the previous one, closes the gate.
    frame.dispatchEvent(new Event("load"));
    posted.length = 0;
    const t1 = { id: "t1", status: "open", version_n: 2, anchor: { file: "index.html", kind: "text", quote: "q" } };
    const { upsert } = await import("../threads");
    ctl.commentsUi.upsert(t1 as Parameters<typeof upsert>[1]);
    ctl.toggleComment();
    await new Promise(r => setTimeout(r, 50));
    expect(posted).toEqual([]);
    hello(win);
    expect(posted.map(m => m.type)).toEqual(expect.arrayContaining(["clax:welcome", "clax:resolve-anchors", "clax:focus"]));
    expect(posted.find(m => m.type === "clax:resolve-anchors")!.anchors).toHaveLength(1);
    ctl.dispose();
  });

  it("sends nothing, answers nothing and forgets the page after the frame's document says bye, until the next hello", async () => {
    const { ctl, frame } = await started();
    const win = frame.contentWindow!;
    const posted: { type: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    hello(win);
    expect(ctl.state.get().file).toBe("index.html");
    // As it arrives in Chromium: once the next document replaced the page, with no source.
    dispatchTrusted(window, new MessageEvent("message", { data: { type: "clax:bye" }, origin: "null", source: null }));
    expect(ctl.state.get().file).toBeNull();
    posted.length = 0;
    // The frame now shows another site's document, which has not loaded yet.
    ctl.toggleComment();
    fromFrame(win, { type: "clax:use", id: "u1", name: "storage" });
    await new Promise(r => setTimeout(r, 50));
    expect(posted).toEqual([]);
    hello(win);
    expect(posted.map(m => m.type)).toContain("clax:welcome");
    expect(ctl.state.get().file).toBe("index.html");
    ctl.dispose();
  });

  it("ignores a bye that a script made rather than the browser delivered", async () => {
    const { ctl, frame } = await started();
    const win = frame.contentWindow!;
    win.postMessage = (() => {}) as Window["postMessage"];
    hello(win);
    window.dispatchEvent(new MessageEvent("message", { data: { type: "clax:bye" }, origin: "null", source: win }));
    expect(ctl.state.get().file).toBe("index.html");
    ctl.dispose();
  });

  it("maps anchor results through the handles of the page greeted when they arrive", async () => {
    const { ctl, frame } = await started();
    const win = frame.contentWindow!;
    const posted: { type: string; anchors?: { id: string }[] }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    const t1 = { id: "t1", status: "open", version_n: 2, anchor: { file: "index.html", kind: "text", quote: "q" } };
    const { upsert } = await import("../threads");
    ctl.commentsUi.upsert(t1 as Parameters<typeof upsert>[1]);
    hello(win);
    const handle = posted.filter(m => m.type === "clax:resolve-anchors").at(-1)!.anchors![0].id;
    fromFrame(win, { type: "clax:anchors", results: [{ id: handle, found: true, method: "quote" }] });
    expect(ctl.state.get().resolved).toEqual({ t1: { id: "t1", found: true, method: "quote" } });
    // A later greeting forgets the handle: its results no longer map back.
    hello(win);
    expect(ctl.state.get().resolved).toEqual({});
    fromFrame(win, { type: "clax:anchors", results: [{ id: handle, found: true, method: "quote" }] });
    expect(ctl.state.get().resolved).toEqual({});
    ctl.dispose();
  });

  it("seeds the working list from the artifact, and a working event replaces it without touching the data", async () => {
    const w = { key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: ["t1"], started_at: "2026-09-30T10:00:00.000Z", last_heartbeat: "2026-09-30T10:00:00.000Z" };
    const { ctl } = await started({ artifact: { working: [w] }, attention: { addressed: [], addressed_v: null, new_replies: [], open_in: ["t1"], seen: null, looked: {} } });
    expect(ctl.state.get().working).toEqual([w]);
    expect(ctl.state.get().attention?.open_in).toEqual(["t1"]);
    const data = ctl.state.get().data;
    const deadline = Date.now() + 2000;
    while (!FakeES.last && Date.now() < deadline) await new Promise(r => setTimeout(r, 5));
    const next = { ...w, key: "k2", message: "Two columns" };
    FakeES.last!.emit("working", { type: "working", artifact_id: ID, working: [next] });
    expect(ctl.state.get().working).toEqual([next]);
    expect(ctl.state.get().data).toBe(data);
    ctl.dispose();
  });

  it("drops a deleted artifact's frame before the next paint, so nothing the page posts after the deletion is answered", async () => {
    const { ctl, frame } = await started();
    const win = frame.contentWindow!;
    const posted: { type: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    hello(win);
    expect(posted.map(m => m.type)).toContain("clax:welcome");
    const deadline = Date.now() + 2000;
    while (!FakeES.last && Date.now() < deadline) await new Promise(r => setTimeout(r, 5));
    FakeES.last!.emit("artifact_deleted", { type: "artifact_deleted", artifact_id: ID });
    await Promise.resolve();
    expect(frame.isConnected).toBe(false);
    // The reactions still pending from the render before the deletion ran
    // first and may have posted to the page; only what follows matters.
    posted.length = 0;
    fromFrame(win, { type: "clax:use", id: "u1", name: "storage" });
    fromFrame(win, { type: "clax:pick-start", pickId: "p1", anchor: { kind: "element", selector: "p", file: "index.html" }, version: 2 });
    await new Promise(r => setTimeout(r, 50));
    expect(posted).toEqual([]);
    ctl.dispose();
  });

  it("acts on C and ?, Escape closes the sheet before leaving comment mode, and every other key is left to the browser", async () => {
    const { ctl } = await started({ threads: [thread("t1"), thread("t2")] });
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const key = (k: string, init: KeyboardEventInit = {}) => {
      const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
      dispatchEvent(e);
      return e.defaultPrevented;
    };
    const posted = () => vi.mocked(fetch).mock.calls.map(c => String(c[0])).filter(u => /\/(send|resolve)$/.test(u));
    const before = ctl.state.get();
    for (const k of ["t", "j", "k", "s", "r", "x", "v", "p", "Enter"]) expect(key(k), k).toBe(false);
    expect(key("S", { shiftKey: true })).toBe(false);
    expect(ctl.state.get()).toMatchObject({ commenting: before.commenting, panel: before.panel, selected: before.selected, sheet: null });
    expect(key("c")).toBe(true);
    expect(ctl.state.get().commenting).toBe(true);
    expect(key("?", { shiftKey: true })).toBe(true);
    expect(ctl.state.get().sheet).toBe("keys");
    key("Escape");
    expect(ctl.state.get()).toMatchObject({ sheet: null, commenting: true });
    key("Escape");
    expect(ctl.state.get().commenting).toBe(false);
    expect(posted()).toEqual([]);
    ctl.dispose();
  });

  it("ignores C and ? while the sheet, a page's prompt or the composer is open", async () => {
    const { ctl } = await started();
    const key = (k: string, init: KeyboardEventInit = {}) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));
    const still = () => {
      const before = ctl.state.get();
      key("c");
      key("?", { shiftKey: true });
      const after = ctl.state.get();
      expect({ commenting: after.commenting, sheet: after.sheet }).toEqual({ commenting: before.commenting, sheet: before.sheet });
    };
    // The sheet: only Escape acts, and it closes the sheet.
    key("?", { shiftKey: true });
    expect(ctl.state.get().sheet).toBe("keys");
    still();
    key("Escape");
    expect(ctl.state.get().sheet).toBeNull();
    // A page's prompt: it also closes an open sheet.
    key("?", { shiftKey: true });
    const prompt = (ctl as unknown as { prompt(p: { title: string; body: string; allow: string; deny: string }): Promise<string> }).prompt;
    void prompt({ title: "T", body: "B", allow: "Allow", deny: "Don't allow" });
    await vi.waitFor(() => expect(ctl.state.get().ask).not.toBeNull());
    expect(ctl.state.get().sheet).toBeNull();
    still();
    ctl.state.get().ask!.answer("deny");
    await vi.waitFor(() => expect(ctl.state.get().ask).toBeNull());
    // The composer.
    ctl.state.set({ draft: { pickId: "p1" } as unknown as NonNullable<ReturnType<typeof ctl.state.get>["draft"]> });
    still();
    expect(ctl.state.get().sheet).toBeNull();
    ctl.dispose();
  });

  it("closes the sheet with a notice when its code cannot load, so the keys act again", async () => {
    const { ctl } = await started();
    dispatchEvent(new KeyboardEvent("keydown", { key: "?", shiftKey: true, bubbles: true }));
    expect(ctl.state.get().sheet).toBe("keys");
    ctl.sheetFailed();
    expect(ctl.state.get()).toMatchObject({ sheet: null, notice: "Could not open the keyboard shortcuts." });
    dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true }));
    expect(ctl.state.get().commenting).toBe(true);
    ctl.dispose();
  });

  it("holds the shell's keys after a page's prompt or composer closes, until the viewer presses or Tabs onto a shell control", async () => {
    const { ctl } = await started();
    const key = (k: string, init: KeyboardEventInit = {}) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));
    const inert = () => {
      const before = ctl.state.get();
      key("c");
      key("?", { shiftKey: true });
      const after = ctl.state.get();
      expect({ commenting: after.commenting, sheet: after.sheet }).toEqual({ commenting: before.commenting, sheet: before.sheet });
    };
    // The page's prompt opens and is answered by a key: focus falls to the body.
    const prompt = (ctl as unknown as { prompt(p: { title: string; body: string; allow: string; deny: string }): Promise<string> }).prompt;
    void prompt({ title: "T", body: "B", allow: "Allow", deny: "Don't allow" });
    await vi.waitFor(() => expect(ctl.state.get().ask).not.toBeNull());
    ctl.state.get().ask!.answer("dismiss");
    await vi.waitFor(() => expect(ctl.state.get().ask).toBeNull());
    inert();
    // An untrusted press, and a Tab that lands in a dialog, do not count.
    dispatchEvent(new Event("pointerdown"));
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    const inDialog = document.createElement("button");
    dialog.append(inDialog);
    document.body.append(dialog);
    dispatchTrusted(window, new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    dispatchTrusted(inDialog, new FocusEvent("focusin", { bubbles: true }));
    inert();
    // Tab to a shell control gives the keys back.
    const control = document.createElement("button");
    document.body.append(control);
    dispatchTrusted(window, new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true }));
    dispatchTrusted(control, new FocusEvent("focusin", { bubbles: true }));
    key("c");
    expect(ctl.state.get().commenting).toBe(true);
    key("c");
    // A composer the page opened holds them too, until a trusted press.
    expect(ctl.commentsUi.openComposer({ kind: "element", selector: "h2", file: "index.html" } as never, {} as never)).toBe(true);
    ctl.cancelDraft();
    inert();
    dispatchTrusted(window, new Event("pointerdown"));
    key("c");
    expect(ctl.state.get().commenting).toBe(true);
    ctl.dispose();
  });

  it("starts with the keys held, once, after a load the page caused", async () => {
    const { holdKeysAcrossLoad } = await import("./keys");
    holdKeysAcrossLoad();
    const { ctl } = await started();
    const key = (k: string) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
    key("c");
    expect(ctl.state.get().commenting).toBe(false);
    expect(sessionStorage.getItem("clax.keys-held")).toBeNull();
    dispatchTrusted(window, new Event("pointerdown"));
    key("c");
    expect(ctl.state.get().commenting).toBe(true);
    ctl.dispose();
    // The mark is used up: the next view starts with the keys live.
    const next = (await started()).ctl;
    key("c");
    expect(next.state.get().commenting).toBe(true);
    next.dispose();
  });

  it("gives the keys up on every close of a page's prompt and on any loss of window focus, and takes them back only on the viewer's own act", async () => {
    const { ctl } = await started({ threads: [thread("t1"), thread("t2")] });
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const key = (k: string) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
    const live = () => { const before = ctl.state.get().commenting; key("c"); const now = ctl.state.get().commenting; if (now !== before) key("c"); return now !== before; };
    const focusOn = (el: Element) => dispatchTrusted(el, new FocusEvent("focusin", { bubbles: true }));
    const el = (html: string) => { const box = document.createElement("div"); box.innerHTML = html; document.body.append(box); return box.firstElementChild!.querySelector("[data-at]") ?? box.firstElementChild!; };
    expect(live()).toBe(true);
    // A press while the prompt is open (on its button, say) does not outlast the prompt.
    const prompt = (ctl as unknown as { prompt(p: { title: string; body: string; allow: string; deny: string }): Promise<string> }).prompt;
    void prompt({ title: "T", body: "B", allow: "Allow", deny: "Don't allow" });
    await vi.waitFor(() => expect(ctl.state.get().ask).not.toBeNull());
    dispatchTrusted(window, new Event("pointerdown"));
    ctl.state.get().ask!.answer("deny");
    await vi.waitFor(() => expect(ctl.state.get().ask).toBeNull());
    expect(live()).toBe(false);
    // Focus on the body, in a dialog or in the composer (where the shell's
    // own script puts it for the page's UI) does not count.
    focusOn(document.body);
    focusOn(el(`<div role="dialog"><button data-at></button></div>`));
    focusOn(el(`<div class="composer"><textarea data-at></textarea></div>`));
    expect(live()).toBe(false);
    // Focus landing on a specific shell control does: the page cannot put it there.
    focusOn(el(`<button>Threads</button>`));
    expect(live()).toBe(true);
    // The window losing focus, to the frame or anywhere else, gives them up.
    dispatchTrusted(window, new FocusEvent("blur"));
    expect(live()).toBe(false);
    dispatchTrusted(window, new Event("pointerdown"));
    expect(live()).toBe(true);
    ctl.dispose();
  });

  it("hands focus the page pushed out to the shell's body back to the frame: at once on the viewer's next key, which is swallowed, or a task later", async () => {
    const { ctl, frame } = await started();
    const tick = () => new Promise(r => setTimeout(r, 0));
    // The frame has focus; the page calls parent.focus(): the window's blur, focus on <body>, the window's focus.
    const pushOut = () => {
      frame.focus();
      expect(document.activeElement).toBe(frame);
      dispatchTrusted(window, new FocusEvent("blur"));
      (document.activeElement as HTMLElement).blur();
      expect(document.activeElement).toBe(document.body);
      dispatchTrusted(window, new FocusEvent("focus"));
    };
    const heard = vi.fn();
    addEventListener("keydown", heard);
    // No key: a task later.
    pushOut();
    expect(document.activeElement).toBe(document.body);
    await tick();
    expect(document.activeElement).toBe(frame);
    // The viewer's keys arrive first (a page busy after parent.focus()): the
    // first is swallowed, reaches none of the shell's listeners, and focus is
    // back on the frame before it returns. Tab, Space and letters alike.
    for (const key of ["Tab", " ", "c"]) {
      pushOut();
      const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      dispatchTrusted(document.body, e);
      expect(e.defaultPrevented, key).toBe(true);
      expect(document.activeElement, key).toBe(frame);
      await tick();
    }
    expect(heard).not.toHaveBeenCalled();
    expect(ctl.state.get().commenting).toBe(false);
    // An untrusted key does nothing.
    pushOut();
    const fake = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    document.body.dispatchEvent(fake);
    expect(fake.defaultPrevented).toBe(false);
    await tick();
    // A trusted press in the shell cancels it: focus stays on the body, and keys are the shell's again.
    pushOut();
    dispatchTrusted(window, new Event("pointerdown"));
    const after = new KeyboardEvent("keydown", { key: "x", bubbles: true, cancelable: true });
    dispatchTrusted(document.body, after);
    expect(after.defaultPrevented).toBe(false);
    await tick();
    expect(document.activeElement).toBe(document.body);
    // The viewer's own Tab out of the frame lands on a shell control: that cancels it, and it stays there.
    const button = document.body.appendChild(document.createElement("button"));
    pushOut();
    button.focus();
    dispatchTrusted(button, new FocusEvent("focusin", { bubbles: true }));
    await tick();
    expect(document.activeElement).toBe(button);
    // A blur while a shell control had focus is not the frame's: no give-back, and keys pass.
    dispatchTrusted(window, new FocusEvent("blur"));
    button.blur();
    dispatchTrusted(window, new FocusEvent("focus"));
    const own = new KeyboardEvent("keydown", { key: "x", bubbles: true, cancelable: true });
    dispatchTrusted(document.body, own);
    expect(own.defaultPrevented).toBe(false);
    await tick();
    expect(document.activeElement).toBe(document.body);
    removeEventListener("keydown", heard);
    ctl.dispose();
  });

  it("taints the keyboard trail when focus enters the shell from the frame or the body without a press, and clears it on a press or an Escape on a control", async () => {
    const { ctl, frame } = await started();
    const { keyboardTrail } = await import("./trail");
    const button = document.body.appendChild(document.createElement("button"));
    const other = document.body.appendChild(document.createElement("button"));
    const land = (el: Element, from: Element | null) => { (el as HTMLElement).focus(); dispatchTrusted(el, new FocusEvent("focusin", { bubbles: true, relatedTarget: from })); };
    expect(keyboardTrail.tainted).toBe(false);
    // From the frame (the viewer's Tab out, or the page running out of fields).
    land(button, frame);
    expect(keyboardTrail.tainted).toBe(true);
    // A Tab on to another control keeps it.
    land(other, button);
    expect(keyboardTrail.tainted).toBe(true);
    // An Escape with focus on <body> does not clear it; on a control it does.
    other.blur();
    dispatchTrusted(document.body, new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(keyboardTrail.tainted).toBe(true);
    other.focus();
    dispatchTrusted(other, new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(keyboardTrail.tainted).toBe(false);
    // An untrusted Escape never clears it.
    land(button, null);
    expect(keyboardTrail.tainted).toBe(true);
    button.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(keyboardTrail.tainted).toBe(true);
    // A press clears it, and the focus that press gives a control does not taint it.
    dispatchTrusted(window, new Event("pointerdown"));
    land(other, null);
    expect(keyboardTrail.tainted).toBe(false);
    await new Promise(r => setTimeout(r, 0));
    // Focus the browser puts back when the window regains focus changes nothing.
    dispatchTrusted(window, new FocusEvent("blur"));
    land(other, null);
    expect(keyboardTrail.tainted).toBe(false);
    // Focus arriving from <body> after that taints it again.
    land(button, null);
    expect(keyboardTrail.tainted).toBe(true);
    ctl.dispose();
  });

  it("lets keys through when the frame a give-back waits for is gone or replaced, and when the viewer types in the composer", async () => {
    const { ctl, frame } = await started();
    const pushOut = () => {
      ctl.frame!.el!.focus();
      dispatchTrusted(window, new FocusEvent("blur"));
      (document.activeElement as HTMLElement).blur();
      dispatchTrusted(window, new FocusEvent("focus"));
    };
    const key = (k: string, at: Element = document.body) => { const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }); dispatchTrusted(at, e); return e; };
    pushOut();
    frame.remove();
    expect(key("Tab").defaultPrevented).toBe(false);
    expect(key("Tab").defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(document.body);
    // A replaced frame: the give-back was for the old one.
    const stage = document.body.appendChild(document.createElement("div"));
    stage.append(frame);
    pushOut();
    const next = document.createElement("iframe");
    frame.replaceWith(next);
    ctl.frame!.el = next;
    expect(key("Tab").defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(document.body);
    // The viewer's own keys in the composer end the give-back: the Escape that
    // closes it, then the next Escape, reach the shell.
    pushOut();
    const composer = document.body.appendChild(document.createElement("div"));
    composer.className = "composer";
    const ta = composer.appendChild(document.createElement("textarea"));
    ta.focus();
    key("Escape", ta);
    composer.remove();
    expect(document.activeElement).toBe(document.body);
    expect(key("Escape").defaultPrevented).toBe(false);
    ctl.dispose();
  });

  it("after a load the page caused, starts with the trail tainted and a give-back pending to the new frame", async () => {
    const { holdKeysAcrossLoad } = await import("./keys");
    holdKeysAcrossLoad();
    const { ctl, frame } = await started();
    const { keyboardTrail } = await import("./trail");
    expect(keyboardTrail.tainted).toBe(true);
    (document.activeElement as HTMLElement | null)?.blur?.();
    const e = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    dispatchTrusted(document.body, e);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(frame);
    ctl.dispose();
  });

  describe("batch send", () => {
    const batchSeed: Seed = {
      threads: [thread("t1"), thread("t2")],
      artifact: { participants: { people: [], agents: [{ handle: "a_cl", harness: "claude", live: true }] } },
      routes: (url, init) => (init?.method === "POST" && String(url).endsWith("/threads:send")
        ? { threads: [thread("t1", { sent_to_agent: true }), thread("t2", { sent_to_agent: true })], sent: ["t1", "t2"], unchanged: [] }
        : undefined),
    };
    afterEach(() => localStorage.clear());

    it("ticks a range, sends it as one batch to the default agent with the note, then clears", async () => {
      const { ctl } = await started(batchSeed);
      await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
      const [t1, t2] = ctl.state.get().threads;
      expect(ctl.state.get().sendTo).toBe("a_cl");
      ctl.toggleSelect(t1, false);
      ctl.toggleSelect(t2, true);
      ctl.setBatchNote("Before the demo");
      await ctl.sendSelection();
      const call = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.find(([u]) => String(u).endsWith("/threads:send"))!;
      expect(JSON.parse((call[1] as RequestInit).body as string)).toEqual({ thread_ids: ["t1", "t2"], note: "Before the demo", to: "a_cl" });
      expect(ctl.state.get()).toMatchObject({ selection: { ids: [], anchor: null }, batchNote: "", batchBusy: false });
      ctl.dispose();
    });

    it("drops a thread from the selection when it disappears", async () => {
      const { ctl } = await started(batchSeed);
      await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
      const [t1, t2] = ctl.state.get().threads;
      ctl.toggleSelect(t1, false);
      ctl.toggleSelect(t2, false);
      FakeES.last!.emit("thread_deleted", { type: "thread_deleted", artifact_id: ID, thread_id: "t1" });
      await Promise.resolve();
      expect(ctl.state.get().selection.ids).toEqual(["t2"]);
      ctl.dispose();
    });

    it("names no target with no live agent, and sends the batch without to", async () => {
      const { ctl } = await started({ ...batchSeed, artifact: { participants: { people: [], agents: [] } } });
      await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
      const [t1, t2] = ctl.state.get().threads;
      expect(ctl.state.get().sendTo).toBeNull();
      ctl.toggleSelect(t1, false);
      ctl.toggleSelect(t2, false);
      await ctl.sendSelection();
      const call = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.find(([u]) => String(u).endsWith("/threads:send"))!;
      expect(JSON.parse((call[1] as RequestInit).body as string)).toEqual({ thread_ids: ["t1", "t2"], note: null });
      ctl.dispose();
    });
  });
});
