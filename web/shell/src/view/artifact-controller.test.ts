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

describe("ArtifactController", () => {
  beforeEach(() => { vi.resetModules(); FakeES.last = undefined; history.replaceState(null, "", `/a/${ID}`); });
  // The gesture module this test's registry loaded watches the document until its `unwatchShell` runs.
  afterEach(async () => { (await import("../caps/gesture")).unwatchShell(); vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); });

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

  it("acts on shell keys: C, T, J and K, ?, and Escape closes the sheet before leaving comment mode", async () => {
    const { ctl } = await started({ threads: [thread("t1"), thread("t2")] });
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const key = (k: string, init: KeyboardEventInit = {}) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));
    key("c");
    expect(ctl.state.get().commenting).toBe(true);
    const panel = ctl.state.get().panel;
    key("t");
    expect(ctl.state.get().panel).toBe(!panel);
    key("j");
    expect(ctl.state.get().selected).toBe(ctl.state.get().threads[0].id);
    key("j");
    expect(ctl.state.get().selected).toBe(ctl.state.get().threads[1].id);
    key("k");
    expect(ctl.state.get().selected).toBe(ctl.state.get().threads[0].id);
    key("?", { shiftKey: true });
    expect(ctl.state.get().sheet).toBe("keys");
    key("Escape");
    expect(ctl.state.get()).toMatchObject({ sheet: null, commenting: true });
    key("Escape");
    expect(ctl.state.get().commenting).toBe(false);
    ctl.dispose();
  });

  it("acts on Enter, S and R for the selected thread, and leaves keys that do nothing to the browser", async () => {
    const { ctl } = await started({ threads: [thread("t1"), thread("t2")] });
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const key = (k: string, init: KeyboardEventInit = {}) => {
      const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
      dispatchEvent(e);
      return e.defaultPrevented;
    };
    const posted = () => vi.mocked(fetch).mock.calls.map(c => String(c[0])).filter(u => /\/(send|resolve)$/.test(u));
    // Nothing selected: Enter, S and R do nothing and keep their default.
    expect([key("Enter"), key("s"), key("r")]).toEqual([false, false, false]);
    expect(ctl.state.get().replyFocus).toBe(0);
    // A key with no action yet (Shift+S, ticked threads) is not swallowed.
    expect(key("S", { shiftKey: true })).toBe(false);
    expect(key("j")).toBe(true);
    expect(key("Enter")).toBe(true);
    expect(ctl.state.get()).toMatchObject({ panel: true, replyFocus: 1 });
    expect(key("s")).toBe(true);
    expect(key("R")).toBe(true);
    await vi.waitFor(() => expect(posted()).toEqual([`/api/artifacts/${ID}/threads/t1/send`, `/api/artifacts/${ID}/threads/t1/resolve`]));
    ctl.dispose();
  });

  it("ignores every shell key while the sheet, a page's prompt or the composer is open", async () => {
    const { ctl } = await started({ threads: [thread("t1"), thread("t2")] });
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const key = (k: string, init: KeyboardEventInit = {}) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));
    key("j");
    const posted = () => vi.mocked(fetch).mock.calls.map(c => String(c[0])).filter(u => /\/(send|resolve)$/.test(u));
    const still = () => {
      const before = ctl.state.get();
      for (const k of ["s", "r", "c", "t", "j", "k", "Enter"]) key(k);
      key("?", { shiftKey: true });
      const after = ctl.state.get();
      expect({ commenting: after.commenting, panel: after.panel, selected: after.selected, replyFocus: after.replyFocus })
        .toEqual({ commenting: before.commenting, panel: before.panel, selected: before.selected, replyFocus: before.replyFocus });
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
    expect(ctl.state.get().sheet).toBeNull();
    ctl.state.get().ask!.answer("deny");
    await vi.waitFor(() => expect(ctl.state.get().ask).toBeNull());
    // The composer.
    ctl.state.set({ draft: { pickId: "p1" } as unknown as NonNullable<ReturnType<typeof ctl.state.get>["draft"]> });
    still();
    expect(ctl.state.get().sheet).toBeNull();
    expect(posted()).toEqual([]);
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

  it("holds the shell's keys after a page's prompt or composer closes, until the viewer presses or Tabs in the shell", async () => {
    const { ctl } = await started({ threads: [thread("t1"), thread("t2")] });
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const key = (k: string, init: KeyboardEventInit = {}) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));
    const posted = () => vi.mocked(fetch).mock.calls.map(c => String(c[0])).filter(u => /\/(send|resolve)$/.test(u));
    const inert = () => {
      const before = ctl.state.get();
      for (const k of ["s", "r", "c", "t", "j"]) key(k);
      key("?", { shiftKey: true });
      const after = ctl.state.get();
      expect({ commenting: after.commenting, panel: after.panel, selected: after.selected, sheet: after.sheet })
        .toEqual({ commenting: before.commenting, panel: before.panel, selected: before.selected, sheet: before.sheet });
      expect(posted()).toEqual([]);
    };
    key("j");
    expect(ctl.state.get().selected).toBe("t1");
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
    key("j");
    expect(ctl.state.get().selected).toBe("t2");
    // A composer the page opened holds them too, until a trusted press.
    expect(ctl.commentsUi.openComposer({ kind: "element", selector: "h2", file: "index.html" } as never, {} as never)).toBe(true);
    ctl.cancelDraft();
    inert();
    dispatchTrusted(window, new Event("pointerdown"));
    key("c");
    expect(ctl.state.get().commenting).toBe(true);
    ctl.dispose();
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
});
