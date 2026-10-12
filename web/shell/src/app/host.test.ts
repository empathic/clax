import { afterEach, describe, expect, it, vi } from "vitest";
import { MOUNT_TIMEOUT_MS } from "../test/timeouts";
import { FakeES, ID, artifact, artifactViewHooks, buttonNamed, fromFrame, gesture, gestureIn, mountView, page, pick, startOf, viewerPick, waitFor } from "../test/artifact-view";
import type { HostMessage } from "./host";

type Posted = { type: string; [k: string]: unknown };
const settle = () => new Promise(r => setTimeout(r, 30));
const thread = (id: string, body = "make it blue", over: Record<string, unknown> = {}) => ({
  id, artifact_id: ID, version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "2026-10-12T10:00:00.000Z",
  resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: null, via_harness: null, body, created_at: "2026-10-12T10:00:00.000Z" }],
  anchor: { kind: "element", selector: "body > h2", quote: "Quarterly goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }, ...over,
});

/** The artifact's latest version, as the daemon answers it now. */
let latest = 1;
/** Every request the view made, in order. */
let requests: { url: string; method: string; body: unknown }[] = [];
/** The app's channel: every message the view posted to its host. */
let toHost: HostMessage[] = [];
/** While set, the artifact's answers wait for it (each answers with the
 * version that was latest when it was asked). */
let held: Promise<void> | null = null;

async function appView(opts: { pinned?: number | null; channel?: boolean; threads?: unknown[]; comments?: (url: string, init?: RequestInit) => Response | undefined } = {}) {
  history.replaceState(null, "", opts.pinned ? `/a/${ID}/v/${opts.pinned}?app=1` : `/a/${ID}?app=1`);
  toHost = [];
  requests = [];
  if (opts.channel !== false) (window as unknown as { ClaxHost?: unknown }).ClaxHost = { postMessage: (m: string) => toHost.push(JSON.parse(m)) };
  const note = (url: string, init?: RequestInit) => {
    const raw = init?.body;
    requests.push({ url, method: init?.method ?? "GET", body: typeof raw === "string" ? JSON.parse(raw) : raw ?? null });
  };
  const view = await mountView(async (url, init) => {
    note(url, init);
    const n = latest;
    if (held) await held;
    return new Response(JSON.stringify(artifact(n, { "index.html": page })));
  }, async (url, init) => {
    note(url, init);
    const own = opts.comments?.(url, init);
    if (own) return own;
    if (url.startsWith("/api/viewers/")) return new Response(JSON.stringify({ viewer: { public_id: "u_0123456789abcdef012345", display_name: null, created_at: "x" } }));
    return new Response(JSON.stringify({ threads: opts.threads ?? [], next_cursor: null }));
  }, undefined, opts.pinned ?? null);
  const ctl = view.controller();
  const m = await import("./host");
  const uninstall = m.installHost(window, ctl);
  const frame = await waitFor(() => view.root.querySelector<HTMLIFrameElement>("iframe.frame"), "frame");
  const host = (window as unknown as { claxHost: import("./host").ClaxHost }).claxHost;
  return { view, ctl, frame, host, uninstall, m };
}

/** The frame's posted messages, kept as they go. */
function framePosts(frame: HTMLIFrameElement): Posted[] {
  const posted: Posted[] = [];
  frame.contentWindow!.postMessage = ((m: Posted) => { posted.push(m); }) as Window["postMessage"];
  return posted;
}
const hello = (frame: HTMLIFrameElement, version: number, file = "index.html") => fromFrame(frame.contentWindow!, { type: "clax:hello", artifact: ID, version, file });
const ofType = <T extends HostMessage["type"]>(t: T) => toHost.filter((m): m is Extract<HostMessage, { type: T }> => m.type === t);

describe("app mode's host", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  afterEach(() => {
    delete (window as unknown as { ClaxHost?: unknown }).ClaxHost;
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
    document.documentElement.classList.remove("app");
    latest = 1;
    held = null;
  });

  it("posts ready with the artifact and version on each page that greets, and marks the document as the app's", async () => {
    latest = 2;
    const { frame } = await appView();
    expect(document.documentElement.classList.contains("app")).toBe(true);
    expect(ofType("ready")).toEqual([]);
    hello(frame, 2);
    await waitFor(() => ofType("ready").length === 1, "ready");
    expect(ofType("ready")[0]).toEqual({ type: "ready", artifact: ID, version: 2, latest: 2, file: "index.html" });
    // The next document the frame loads greets again.
    fromFrame(frame.contentWindow!, { type: "clax:bye" });
    hello(frame, 2);
    expect(ofType("ready")).toHaveLength(2);
  });

  it("welcomes every page with app: true", async () => {
    const { frame } = await appView();
    const posted = framePosts(frame);
    hello(frame, 1);
    expect(posted.find(m => m.type === "clax:welcome")).toEqual({ type: "clax:welcome", mode: "view", app: true });
  });

  it("enterCommentMode and exitCommentMode set commenting once", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    host.enterCommentMode();
    expect(ctl.state.get().commenting).toBe(true);
    host.exitCommentMode();
    host.exitCommentMode();
    expect(ctl.state.get().commenting).toBe(false);
    expect(ofType("state").map(m => m.commenting)).toEqual([false, true, false]);
  });

  it("state follows the draft: label, text, clip pending then ready", async () => {
    const { frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    gestureIn(frame);
    const p = pick("p1", "Quarterly goals");
    fromFrame(frame.contentWindow!, startOf(p));
    await waitFor(() => ofType("state").at(-1)?.draft, "a draft");
    expect(ofType("state").at(-1)).toEqual({ type: "state", commenting: false, draft: { label: "«Quarterly goals»", text: "", clip: "pending" }, busy: false });
    const textarea = await waitFor(() => document.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    textarea.value = "bigger";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    expect(ofType("state").at(-1)!.draft).toEqual({ label: "«Quarterly goals»", text: "bigger", clip: "pending" });
    fromFrame(frame.contentWindow!, { ...p, clipPng: new Uint8Array([1, 2, 3]).buffer });
    expect(ofType("state").at(-1)!.draft).toEqual({ label: "«Quarterly goals»", text: "bigger", clip: "ready" });
    host.cancelDraft();
    // Comment mode comes back once the composer has closed.
    await waitFor(() => ofType("state").at(-1)?.commenting, "comment mode back");
    expect(ofType("state").at(-1)).toEqual({ type: "state", commenting: true, draft: null, busy: false });
  });

  it("a draft whose screenshot failed has clip none", async () => {
    const { frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipError: "too big" });
    await waitFor(() => ofType("state").at(-1)?.draft?.clip === "none", "no clip");
  });

  it("setDraftText replaces an open composer's text and is a no-op without one", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.setDraftText("nothing open");
    expect(ctl.state.get().draft).toBeNull();
    const before = toHost.length;
    expect(toHost.length).toBe(before);
    host.enterCommentMode();
    viewerPick(frame, pick("p1", "Quarterly goals"));
    const textarea = await waitFor(() => document.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    textarea.value = "typed";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    host.setDraftText("make it blue");
    expect(textarea.value).toBe("make it blue");
    expect(ctl.composerText).toBe("make it blue");
    expect(ofType("state").at(-1)!.draft!.text).toBe("make it blue");
  });

  it("submitDraft posts the thread, sends it to the agent, and posts sent", async () => {
    const posted = thread("t9");
    const { ctl, frame, host } = await appView({
      comments: (url, init) => {
        if (init?.method !== "POST") return undefined;
        if (url === `/api/artifacts/${ID}/threads`) return new Response(JSON.stringify({ thread: posted }));
        if (url === `/api/artifacts/${ID}/threads/t9/send`) return new Response(JSON.stringify({ thread: { ...posted, sent_to_agent: true } }));
        return undefined;
      },
    });
    hello(frame, 1);
    ctl.chooseTarget("app-agent");
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipError: "none here" });
    await waitFor(() => document.querySelector(".composer textarea"), "composer");
    // The composer's primary button is Send in app mode.
    expect(buttonNamed(document.body, "Send")).toBeTruthy();
    host.setDraftText("make it blue");
    host.submitDraft();
    await waitFor(() => ofType("sent").length, "sent");
    expect(ofType("sent")).toEqual([{ type: "sent", thread: "t9" }]);
    const posts = requests.filter(r => r.method === "POST");
    expect(posts.map(r => r.url)).toEqual([`/api/artifacts/${ID}/threads`, `/api/artifacts/${ID}/threads/t9/send`]);
    const form = posts[0].body as FormData;
    expect(form.get("body")).toBe("make it blue");
    expect(JSON.parse(form.get("anchor") as string)).toMatchObject({ quote: "Quarterly goals", selector: "body > h2" });
    expect(posts[1].body).toEqual({ to: "app-agent" });
    expect(ctl.state.get().draft).toBeNull();
    await waitFor(() => ctl.state.get().threads.find(t => t.id === "t9")?.sent_to_agent, "the sent thread");
    expect(ofType("threads").at(-1)!.threads).toEqual([{ id: "t9", status: "open", summary: "make it blue", sent: true, comments: 1, version: 1 }]);
  });

  it("a send that fails shows the notice and posts no sent", async () => {
    const posted = thread("t9");
    const { frame, host } = await appView({
      comments: (url, init) => {
        if (init?.method !== "POST") return undefined;
        if (url.endsWith("/send")) return new Response(JSON.stringify({ error: { code: "internal", message: "down" } }), { status: 500 });
        return new Response(JSON.stringify({ thread: posted }));
      },
    });
    hello(frame, 1);
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipError: "none here" });
    await waitFor(() => document.querySelector(".composer textarea"), "composer");
    host.setDraftText("x");
    host.submitDraft();
    await waitFor(() => ofType("notice").length, "notice");
    expect(ofType("notice")[0].text).toContain("Could not send to the agent");
    expect(ofType("sent")).toEqual([]);
  });

  it("loadVersion(n) shows version n and loadVersion(null) the latest, keeping app mode", async () => {
    latest = 3;
    const { frame, host } = await appView();
    hello(frame, 3);
    const { nav } = await import("../nav");
    const assign = vi.spyOn(nav, "assign").mockImplementation(() => {});
    host.loadVersion(1);
    host.loadVersion(null);
    host.loadVersion(-1);
    expect(assign.mock.calls.map(c => c[0])).toEqual([`/a/${ID}/v/1?app=1`, `/a/${ID}?app=1`]);
  });

  it("keeps ?app=1 in the address bar as the frame moves between pages", async () => {
    latest = 1;
    const { frame } = await appView();
    hello(frame, 1);
    // A link the page handed over: one history entry, query kept.
    fromFrame(frame.contentWindow!, { type: "clax:navigate", file: "index.html", hash: "#b" });
    expect(location.pathname + location.search + location.hash).toBe(`/a/${ID}?app=1#b`);
    // A page that greeted with another file: the address bar follows, query kept.
    hello(frame, 1, "other.html");
    await settle();
    expect(location.search).toBe("?app=1");
  });

  it("auto_follow_loads_a_newer_version_and_asks_the_bridge_to_restore_scroll", async () => {
    const { ctl, view, frame } = await appView();
    hello(frame, 1);
    fromFrame(frame.contentWindow!, { type: "clax:scroll", file: "index.html", y: 1200 });
    // A report for another page than the one shown is not kept.
    fromFrame(frame.contentWindow!, { type: "clax:scroll", file: "other.html", y: 5 });
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    const next = await waitFor(() => { const f = view.root.querySelector<HTMLIFrameElement>("iframe.frame"); return f && f !== frame ? f : null; }, "the newer version's frame");
    expect(next.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    expect(ctl.state.get().newer).toBeNull();
    const posted = framePosts(next);
    expect(ofType("landed")).toEqual([]);
    hello(next, 2);
    expect(posted.filter(m => m.type === "clax:welcome")).toEqual([{ type: "clax:welcome", mode: "view", app: true, restoreScroll: 1200 }]);
    expect(ofType("landed")).toEqual([{ type: "landed", version: 2 }]);
    expect(ofType("ready").at(-1)).toEqual({ type: "ready", artifact: ID, version: 2, latest: 2, file: "index.html" });
    // A later welcome to the same document (a re-greet) carries no restore.
    hello(next, 2);
    expect(posted.filter(m => m.type === "clax:welcome").at(-1)).toEqual({ type: "clax:welcome", mode: "view", app: true });
    expect(ofType("landed")).toHaveLength(1);
  });

  it("follows a version published while the one before it loads", async () => {
    const { ctl, view, frame } = await appView();
    hello(frame, 1);
    let release!: () => void;
    held = new Promise(r => { release = r; });
    latest = 2;
    const es = await waitFor(() => FakeES.last, "stream");
    es.emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => requests.filter(r => r.url === `/api/artifacts/${ID}`).length >= 2, "the follow's request");
    latest = 3;
    es.emit("version", { type: "version", artifact_id: ID, n: 3 });
    await waitFor(() => ctl.state.get().newer === 3, "newer 3");
    held = null;
    release();
    await waitFor(() => view.root.querySelector("iframe.frame")?.getAttribute("src") === `/c/${ID}/v/3/`, "version 3's frame");
    expect(frame.isConnected).toBe(false);
    expect(ctl.state.get().newer).toBeNull();
  });

  it("auto-follow without a reported scroll asks for no restore", async () => {
    const { view, frame } = await appView();
    hello(frame, 1);
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    const next = await waitFor(() => { const f = view.root.querySelector<HTMLIFrameElement>("iframe.frame"); return f && f !== frame ? f : null; }, "the newer version's frame");
    const posted = framePosts(next);
    hello(next, 2);
    expect(posted.find(m => m.type === "clax:welcome")).toEqual({ type: "clax:welcome", mode: "view", app: true });
  });

  it("auto_follow_waits_for_an_open_draft", async () => {
    const { ctl, view, frame, host } = await appView();
    hello(frame, 1);
    fromFrame(frame.contentWindow!, { type: "clax:scroll", file: "index.html", y: 300 });
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipError: "none" });
    await waitFor(() => document.querySelector(".composer textarea"), "composer");
    host.setDraftText("half a thought");
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ctl.state.get().newer === 2, "newer");
    await settle();
    expect(view.root.querySelector("iframe.frame")).toBe(frame);
    expect(document.querySelector<HTMLTextAreaElement>(".composer textarea")!.value).toBe("half a thought");
    host.cancelDraft();
    const next = await waitFor(() => { const f = view.root.querySelector<HTMLIFrameElement>("iframe.frame"); return f && f !== frame ? f : null; }, "the newer version's frame");
    const posted = framePosts(next);
    hello(next, 2);
    expect(posted.find(m => m.type === "clax:welcome")).toMatchObject({ restoreScroll: 300 });
    expect(ofType("landed")).toEqual([{ type: "landed", version: 2 }]);
  });

  it("auto_follow_never_moves_a_pinned_view", async () => {
    latest = 1;
    const { ctl, view, frame } = await appView({ pinned: 1 });
    hello(frame, 1);
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ctl.state.get().newer === 2, "newer");
    await settle();
    expect(view.root.querySelector("iframe.frame")).toBe(frame);
    expect(ctl.state.get().pinnedVersion).toBe(1);
    expect(ofType("landed")).toEqual([]);
  });

  it("reloadLatest unpins a preview and loads the latest in place", async () => {
    latest = 2;
    const { ctl, view, frame, host } = await appView({ pinned: 1 });
    hello(frame, 1);
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/1/`);
    host.reloadLatest();
    const next = await waitFor(() => { const f = view.root.querySelector<HTMLIFrameElement>("iframe.frame"); return f && f !== frame ? f : null; }, "the latest version's frame");
    expect(next.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    expect(ctl.state.get().pinnedVersion).toBeNull();
    expect(location.pathname + location.search).toBe(`/a/${ID}?app=1`);
    hello(next, 2);
    expect(ofType("landed")).toEqual([{ type: "landed", version: 2 }]);
  });

  it("openThread selects the thread", async () => {
    const { ctl, frame, host } = await appView({ threads: [thread("t1")] });
    hello(frame, 1);
    await waitFor(() => ctl.state.get().threads.length, "threads");
    expect(ofType("threads").at(-1)!.threads).toEqual([{ id: "t1", status: "open", summary: "make it blue", sent: false, comments: 1, version: 1 }]);
    host.openThread("nope");
    expect(ctl.state.get().selected).toBeNull();
    host.openThread("t1");
    expect(ctl.state.get().selected).toBe("t1");
  });

  it("messages are dropped when window.ClaxHost is absent", async () => {
    const { frame, host } = await appView({ channel: false });
    expect(() => { hello(frame, 1); host.enterCommentMode(); }).not.toThrow();
    expect(toHost).toEqual([]);
    // A channel that appears later hears what follows.
    (window as unknown as { ClaxHost?: unknown }).ClaxHost = { postMessage: (m: string) => toHost.push(JSON.parse(m)) };
    host.exitCommentMode();
    expect(ofType("state").at(-1)!.commenting).toBe(false);
  });

  it("uninstall removes claxHost, the hooks and the app class", async () => {
    const { ctl, uninstall } = await appView();
    uninstall();
    expect((window as unknown as { claxHost?: unknown }).claxHost).toBeUndefined();
    expect(ctl.app).toBeNull();
    expect(document.documentElement.classList.contains("app")).toBe(false);
  });
});

describe("app mode's host attestation", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  afterEach(() => {
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    delete (navigator as unknown as { userActivation?: unknown }).userActivation;
    document.documentElement.classList.remove("app");
    vi.restoreAllMocks();
  });

  /** A finger on the page as Android's WebView shows it to the shell: the
   * frame focused and the shell active, with no pointer event the shell saw. */
  function fingerOnPage(frame: HTMLIFrameElement) {
    Object.defineProperty(navigator, "userActivation", { value: { isActive: true }, configurable: true });
    frame.focus();
  }
  /** What is under (x, y): the frame, unless `over` is given. */
  const hitTest = (el: () => Element | null) => { document.elementFromPoint = () => el(); };

  it("counts a host attestation on the frame as the finger's arrival, for ATTEST_MS", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { frame, host, m } = await appView();
    hitTest(() => frame);
    fingerOnPage(frame);
    expect(gesture!.frameGesture()).toBe(false);
    host.attestPointer({ x: 100, y: 100 });
    expect(gesture!.frameGesture()).toBe(true);
    expect(gesture!.frameGestureStrict()).toBe("ok");
    t += m.ATTEST_MS - 1;
    expect(gesture!.frameGesture()).toBe(true);
    t += 1;
    expect(gesture!.frameGesture()).toBe(false);
    // The release, attested too, counts afresh.
    host.attestPointer({ x: 100, y: 120 });
    expect(gesture!.frameGesture()).toBe(true);
  });

  it("a long-press's pick is taken on the attestation alone", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    host.attestPointer({ x: 50, y: 60 });
    fromFrame(frame.contentWindow!, startOf(pick("lp", "Quarterly goals")));
    expect(ctl.state.get().draft?.pickId).toBe("lp");
  });

  it("does not count an attestation over a shell control, a malformed one, or one before the latest shell input", async () => {
    const { frame, host } = await appView();
    const control = document.createElement("button");
    document.body.append(control);
    fingerOnPage(frame);
    hitTest(() => control);
    host.attestPointer({ x: 1, y: 1 });
    expect(gesture!.frameGesture()).toBe(false);
    hitTest(() => frame);
    for (const bad of [null, {}, { x: "1", y: 2 }, { x: NaN, y: 2 }, { x: 1, y: Infinity }]) host.attestPointer(bad as never);
    expect(gesture!.frameGesture()).toBe(false);
    host.attestPointer({ x: 1, y: 1 });
    expect(gesture!.frameGesture()).toBe(true);
    // The viewer's input to the shell after it: no longer the page's gesture.
    gesture!.noteShellInput();
    expect(gesture!.frameGesture()).toBe(false);
  });

  it("still needs the shell's activation and focus in the frame", async () => {
    const { frame, host } = await appView();
    hitTest(() => frame);
    Object.defineProperty(navigator, "userActivation", { value: { isActive: false }, configurable: true });
    frame.focus();
    host.attestPointer({ x: 1, y: 1 });
    expect(gesture!.frameGesture()).toBe(false);
    Object.defineProperty(navigator, "userActivation", { value: { isActive: true }, configurable: true });
    (document.activeElement as HTMLElement | null)?.blur();
    document.body.focus();
    expect(gesture!.frameGesture()).toBe(false);
  });

  it("is not counted outside app mode, nor after uninstall", async () => {
    // A browser's view: no host installed, so no claxHost and the hook is inert.
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))));
    const frame = await waitFor(() => view.root.querySelector<HTMLIFrameElement>("iframe.frame"), "frame");
    expect((window as unknown as { claxHost?: unknown }).claxHost).toBeUndefined();
    hitTest(() => frame);
    fingerOnPage(frame);
    expect(gesture!.hostArrival.fresh(-Infinity)).toBe(false);
    expect(gesture!.frameGesture()).toBe(false);
    const m = await import("./host");
    const uninstall = m.installHost(window, view.controller());
    const host = (window as unknown as { claxHost: import("./host").ClaxHost }).claxHost;
    host.attestPointer({ x: 1, y: 1 });
    expect(gesture!.frameGesture()).toBe(true);
    uninstall();
    expect(gesture!.frameGesture()).toBe(false);
  });

  it("nothing the page posts attests a finger", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    // The page cannot reach claxHost (cross-origin); what it can do is post.
    for (const data of [{ type: "clax:attest", x: 1, y: 1 }, { type: "attestPointer", x: 1, y: 1 }, { type: "clax:scroll", file: "index.html", y: 1 }, { type: "clax:hover", selector: null, rect: null }]) fromFrame(frame.contentWindow!, data);
    expect(gesture!.frameGesture()).toBe(false);
    fromFrame(frame.contentWindow!, startOf(pick("forged", "Forged")));
    expect(ctl.state.get().draft).toBeNull();
  });
});
