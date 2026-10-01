import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

async function started() {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(
    url.includes("/threads") ? { threads: [], next_cursor: null } : url.startsWith("/api/viewers") ? { viewer: { public_id: "u_1", display_name: null, created_at: "x" } } : url === "/api/token" ? { token: "tk" } : loaded))));
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

const fromFrame = (win: Window, data: unknown) => window.dispatchEvent(new MessageEvent("message", { data, origin: "null", source: win }));
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

  it("maps anchor results through the handles of the page greeted when they arrive", async () => {
    const { ctl, frame } = await started();
    const win = frame.contentWindow!;
    const posted: { type: string; anchors?: { id: string }[] }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
    const thread = { id: "t1", status: "open", version_n: 2, anchor: { file: "index.html", kind: "text", quote: "q" } };
    const { upsert } = await import("../threads");
    ctl.commentsUi.upsert(thread as Parameters<typeof upsert>[1]);
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
});
