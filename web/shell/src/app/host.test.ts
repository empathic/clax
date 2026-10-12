import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
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
/** The app's channel: every message the view posted to its host, its nonce
 * checked and taken off. */
let toHost: HostMessage[] = [];
/** The same, as posted. */
let raw: string[] = [];
const NONCE = "n0nce-for-this-load_0123456789";
/** The app's channel as the WebView injects it. */
const channel = () => ({ postMessage: (m: string) => {
  raw.push(m);
  const o = JSON.parse(m) as HostMessage & { nonce?: string };
  if (o.nonce !== NONCE) throw new Error(`a message without this load's nonce: ${m}`);
  delete o.nonce;
  toHost.push(o);
} });
/** While set, the artifact's answers wait for it (each answers with the
 * version that was latest when it was asked). */
let held: Promise<void> | null = null;

async function appView(opts: { pinned?: number | null; channel?: boolean; init?: boolean; threads?: unknown[]; comments?: (url: string, init?: RequestInit) => Response | undefined } = {}) {
  history.replaceState(null, "", opts.pinned ? `/a/${ID}/v/${opts.pinned}?app=1` : `/a/${ID}?app=1`);
  toHost = [];
  raw = [];
  requests = [];
  if (opts.channel !== false) (window as unknown as { ClaxHost?: unknown }).ClaxHost = channel();
  const note = (url: string, init?: RequestInit) => {
    const sent = init?.body;
    requests.push({ url, method: init?.method ?? "GET", body: typeof sent === "string" ? JSON.parse(sent) : sent ?? null });
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
  if (opts.init !== false) host.init({ nonce: NONCE });
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
    expect(ofType("state").at(-1)).toEqual({ type: "state", commenting: false, draft: { label: "“Quarterly goals”", text: "", clip: "pending" }, busy: false });
    const textarea = await waitFor(() => document.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    textarea.value = "bigger";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    expect(ofType("state").at(-1)!.draft).toEqual({ label: "“Quarterly goals”", text: "bigger", clip: "pending" });
    fromFrame(frame.contentWindow!, { ...p, clipPng: new Uint8Array([1, 2, 3]).buffer });
    expect(ofType("state").at(-1)!.draft).toEqual({ label: "“Quarterly goals”", text: "bigger", clip: "ready" });
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
    expect(ofType("threads").at(-1)!.threads).toEqual([{ id: "t9", status: "open", summary: "make it blue", label: "“Quarterly goals”", sent: true, comments: 1, version: 1 }]);
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
    expect(ofType("threads").at(-1)!.threads).toEqual([{ id: "t1", status: "open", summary: "make it blue", label: "“Quarterly goals”", sent: false, comments: 1, version: 1 }]);
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
    (window as unknown as { ClaxHost?: unknown }).ClaxHost = channel();
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
    host.attestPointer({ pointer: 1, phase: "down", x: 100, y: 100 });
    expect(gesture!.frameGesture()).toBe(true);
    expect(gesture!.frameGestureStrict()).toBe("ok");
    t += m.ATTEST_MS - 1;
    expect(gesture!.frameGesture()).toBe(true);
    t += 1;
    expect(gesture!.frameGesture()).toBe(false);
    // The release, attested too, counts afresh.
    host.attestPointer({ pointer: 1, phase: "up", x: 100, y: 120 });
    expect(gesture!.frameGesture()).toBe(true);
  });

  it("a long-press's pick is taken on the attestation alone", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    host.attestPointer({ pointer: 7, phase: "down", x: 50, y: 60 });
    fromFrame(frame.contentWindow!, startOf(pick("lp", "Quarterly goals")));
    expect(ctl.state.get().draft?.pickId).toBe("lp");
  });

  it("does not count an attestation over a shell control, a malformed one, or one before the latest shell input", async () => {
    const { frame, host } = await appView();
    const control = document.createElement("button");
    document.body.append(control);
    fingerOnPage(frame);
    hitTest(() => control);
    host.attestPointer({ pointer: 1, phase: "down", x: 1, y: 1 });
    expect(gesture!.frameGesture()).toBe(false);
    hitTest(() => frame);
    for (const bad of [null, {}, { pointer: 1, phase: "down", x: "1", y: 2 }, { pointer: 1, phase: "down", x: NaN, y: 2 }, { pointer: 1, phase: "down", x: 1, y: Infinity }, { pointer: 1.5, phase: "down", x: 1, y: 1 }, { pointer: 1, phase: "move", x: 1, y: 1 }, { x: 1, y: 1 }]) host.attestPointer(bad as never);
    expect(gesture!.frameGesture()).toBe(false);
    host.attestPointer({ pointer: 2, phase: "down", x: 1, y: 1 });
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
    host.attestPointer({ pointer: 1, phase: "down", x: 1, y: 1 });
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
    // Not at an app shell path: nothing is installed.
    m.installHost(window, view.controller());
    expect((window as unknown as { claxHost?: unknown }).claxHost).toBeUndefined();
    history.replaceState(null, "", `/a/${ID}?app=1`);
    const uninstall = m.installHost(window, view.controller());
    const host = (window as unknown as { claxHost: import("./host").ClaxHost }).claxHost;
    host.attestPointer({ pointer: 1, phase: "down", x: 1, y: 1 });
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

  it("counts a release only when the same pointer's press was attested on the frame", async () => {
    const { frame, host } = await appView();
    const cancel = document.createElement("button");
    document.body.append(cancel);
    fingerOnPage(frame);
    // A tap on a shell control over the frame that is gone by the release.
    hitTest(() => cancel);
    host.attestPointer({ pointer: 1, phase: "down", x: 10, y: 10 });
    cancel.remove();
    hitTest(() => frame);
    host.attestPointer({ pointer: 1, phase: "up", x: 10, y: 10 });
    expect(gesture!.frameGesture()).toBe(false);
    // A release with no press attested at all.
    host.attestPointer({ pointer: 3, phase: "up", x: 10, y: 10 });
    expect(gesture!.frameGesture()).toBe(false);
    // Another pointer's press does not make this one's release count.
    host.attestPointer({ pointer: 4, phase: "down", x: 10, y: 10 });
    gesture!.noteShellInput();
    host.attestPointer({ pointer: 5, phase: "up", x: 10, y: 10 });
    expect(gesture!.frameGesture()).toBe(false);
    // Its own release does, once.
    host.attestPointer({ pointer: 4, phase: "up", x: 10, y: 10 });
    expect(gesture!.frameGesture()).toBe(true);
    gesture!.noteShellInput();
    host.attestPointer({ pointer: 4, phase: "up", x: 10, y: 10 });
    expect(gesture!.frameGesture()).toBe(false);
  });
});

describe("app mode's host channel", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  afterEach(() => {
    delete (window as unknown as { ClaxHost?: unknown }).ClaxHost;
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    document.documentElement.classList.remove("app");
    latest = 1;
  });

  it("posts nothing before init, then the current state, threads and ready, each with the nonce", async () => {
    const { frame, host } = await appView({ init: false, threads: [thread("t1")] });
    const posted = framePosts(frame);
    hello(frame, 1);
    host.enterCommentMode();
    await settle();
    expect(raw).toEqual([]);
    host.init({ nonce: NONCE });
    expect(toHost.map(m => m.type)).toEqual(["state", "threads", "ready"]);
    expect(toHost[0]).toMatchObject({ commenting: true });
    expect(raw.every(m => JSON.parse(m).nonce === NONCE)).toBe(true);
    // Only the first init counts.
    host.init({ nonce: "another-nonce-0123456789" });
    host.exitCommentMode();
    expect(raw.every(m => JSON.parse(m).nonce === NONCE)).toBe(true);
    // The page never hears the nonce.
    expect(JSON.stringify(posted)).not.toContain(NONCE);
  });

  it("ignores an init without a well-formed nonce", async () => {
    const { host } = await appView({ init: false });
    for (const bad of [undefined, {}, { nonce: 5 }, { nonce: "short" }, { nonce: "has spaces in it 0123456789" }, { nonce: "x".repeat(129) }]) host.init(bad as never);
    host.enterCommentMode();
    expect(raw).toEqual([]);
    host.init({ nonce: NONCE });
    expect(toHost.length).toBeGreaterThan(0);
  });

  it("a page that greeted before the host loaded is asked to greet again, and then gets ready and app: true", async () => {
    history.replaceState(null, "", `/a/${ID}?app=1`);
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))));
    const frame = await waitFor(() => view.root.querySelector<HTMLIFrameElement>("iframe.frame"), "frame");
    const posted = framePosts(frame);
    hello(frame, 1);
    expect(posted.find(m => m.type === "clax:welcome")).toEqual({ type: "clax:welcome", mode: "view" });
    (window as unknown as { ClaxHost?: unknown }).ClaxHost = channel();
    toHost = [];
    raw = [];
    const m = await import("./host");
    m.installHost(window, view.controller());
    expect(posted.at(-1)).toEqual({ type: "clax:greet" });
    const from = posted.length;
    hello(frame, 1);
    expect(posted.slice(from).find(x => x.type === "clax:welcome")).toEqual({ type: "clax:welcome", mode: "view", app: true });
    (window as unknown as { claxHost: import("./host").ClaxHost }).claxHost.init({ nonce: NONCE });
    expect(toHost.filter(x => x.type === "ready")).toEqual([{ type: "ready", artifact: ID, version: 1, latest: 1, file: "index.html" }]);
  });

  it("installs nothing outside the top-level document at an app shell path", async () => {
    history.replaceState(null, "", `/a/${ID}`);
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))));
    const m = await import("./host");
    m.installHost(window, view.controller());
    expect((window as unknown as { claxHost?: unknown }).claxHost).toBeUndefined();
    history.replaceState(null, "", `/?app=1`);
    m.installHost(window, view.controller());
    expect((window as unknown as { claxHost?: unknown }).claxHost).toBeUndefined();
    // At the right path, but not the top-level window.
    history.replaceState(null, "", `/a/${ID}?app=1`);
    const framed = new Proxy(window, { get: (t, k) => (k === "top" ? {} : (t as unknown as Record<PropertyKey, unknown>)[k]) });
    m.installHost(framed, view.controller());
    expect((window as unknown as { claxHost?: unknown }).claxHost).toBeUndefined();
    expect(view.controller().app).toBeNull();
    expect(document.documentElement.classList.contains("app")).toBe(false);
  });

  it("a failed send posts sendFailed, and resend sends the thread", async () => {
    const posted = thread("t9");
    let sendOk = false;
    const { ctl, frame, host } = await appView({
      comments: (url, init) => {
        if (init?.method !== "POST") return undefined;
        if (url.endsWith("/send")) return sendOk ? new Response(JSON.stringify({ thread: { ...posted, sent_to_agent: true } })) : new Response(JSON.stringify({ error: { code: "internal", message: "down" } }), { status: 500 });
        return new Response(JSON.stringify({ thread: posted }));
      },
    });
    hello(frame, 1);
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipError: "none" });
    await waitFor(() => document.querySelector(".composer textarea"), "composer");
    host.setDraftText("make it blue");
    host.submitDraft();
    await waitFor(() => toHost.some(x => x.type === "sendFailed"), "sendFailed");
    expect(toHost.find(x => x.type === "sendFailed")).toEqual({ type: "sendFailed", thread: "t9", text: "Could not send to the agent: 500 down" });
    // Not a thread, or one already sent: nothing.
    host.resend("nope");
    sendOk = true;
    host.resend("t9");
    await waitFor(() => toHost.some(x => x.type === "sent"), "sent");
    expect(toHost.filter(x => x.type === "sent")).toEqual([{ type: "sent", thread: "t9" }]);
    await waitFor(() => ctl.state.get().threads.find(t => t.id === "t9")?.sent_to_agent, "sent thread");
    expect(ctl.state.get().notice).toBeNull();
    const sends = () => requests.filter(r => r.url.endsWith("/send")).length;
    const before = sends();
    host.resend("t9");
    await settle();
    expect(sends()).toBe(before);
  });

  it("carries only app=1 from page to page, not the rest of the query", async () => {
    await appView();
    history.replaceState(null, "", `/a/${ID}?app=1&thread=t1`);
    const frame = document.querySelector<HTMLIFrameElement>("iframe.frame")!;
    hello(frame, 1);
    fromFrame(frame.contentWindow!, { type: "clax:navigate", file: "index.html", hash: "#b" });
    expect(location.search).toBe("?app=1");
  });

  it("labels a draft from the pick's element description, and a thread on the whole app", async () => {
    const whole = thread("tw", "all of it", { anchor: { kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" } });
    const { frame, host } = await appView({ threads: [whole] });
    hello(frame, 1);
    await waitFor(() => ofType("threads").at(-1)?.threads.length, "threads");
    expect(ofType("threads").at(-1)!.threads[0].label).toBe("Whole app");
    host.enterCommentMode();
    gestureIn(frame);
    const icon = { kind: "element", selector: "body > main > div.run > svg", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
    fromFrame(frame.contentWindow!, { type: "clax:pick-start", pickId: "d1", version: 1, anchor: icon, desc: { tag: "svg", role: "button", name: "Delete run" } });
    await waitFor(() => ofType("state").at(-1)?.draft, "a draft");
    expect(ofType("state").at(-1)!.draft!.label).toBe("the button “Delete run”");
    // The viewer's own pick is not marked as the page's.
    expect(ofType("state").at(-1)!.draft!.byPage).toBeUndefined();
    expect(JSON.stringify(toHost)).not.toContain("div.run");
    // The shell's own composer says the same, not the selector.
    await waitFor(() => document.querySelector(".composer-quote")?.textContent === "the button “Delete run”", "the composer's words");
  });

  it("refuses a press attestation where the shell itself saw a trusted press moments before", async () => {
    let t = 50_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { frame, host, m } = await appView();
    // A tap on Cancel over the page: the shell saw it, and Cancel is gone by
    // the time the press's attestation runs.
    const cancel = document.createElement("button");
    document.body.append(cancel);
    dispatchTrusted(cancel, new MouseEvent("pointerdown", { bubbles: true, clientX: 200, clientY: 300 }));
    dispatchTrusted(cancel, new MouseEvent("pointerup", { bubbles: true, clientX: 201, clientY: 300 }));
    cancel.remove();
    document.elementFromPoint = () => frame;
    Object.defineProperty(navigator, "userActivation", { value: { isActive: true }, configurable: true });
    frame.focus();
    t += 120;
    host.attestPointer({ pointer: 1, phase: "down", x: 205, y: 310 });
    host.attestPointer({ pointer: 1, phase: "up", x: 205, y: 310 });
    expect(gesture!.frameGesture()).toBe(false);
    // Far from that press, or once ATTEST_MS has passed, a press counts.
    host.attestPointer({ pointer: 2, phase: "down", x: 20, y: 40 });
    expect(gesture!.frameGesture()).toBe(true);
    gesture!.noteShellInput();
    t += m.ATTEST_MS;
    host.attestPointer({ pointer: 3, phase: "down", x: 205, y: 310 });
    expect(gesture!.frameGesture()).toBe(true);
    vi.restoreAllMocks();
  });

  it("labels a composer the page opened without a label, and keeps a label the page gave", async () => {
    const { ctl, frame } = await appView();
    hello(frame, 1);
    const base = { quote: null, prefix: null, suffix: null, html_hash: null, rect: null, file: "index.html" };
    ctl.commentsUi.openComposer({ anchor: { kind: "element", selector: "#save", custom_name: null, ...base }, version: 1, clip: null, clipError: "anchored by the page" });
    await waitFor(() => document.querySelector(".composer-quote")?.textContent === "this spot", "the composer's words");
    expect(ofType("state").at(-1)!.draft).toMatchObject({ label: "this spot", byPage: true });
    expect(JSON.stringify(toHost)).not.toContain("#save");
    ctl.cancelDraft();
    ctl.commentsUi.openComposer({ anchor: { kind: "custom", selector: null, custom_name: "Week 3", ...base }, version: 1, clip: null, clipError: "anchored by the page" });
    await waitFor(() => document.querySelector(".composer-quote")?.textContent === "“Week 3”", "a custom anchor's words");
    ctl.cancelDraft();
    ctl.commentsUi.openComposer({ anchor: { kind: "element", selector: "#save", custom_name: null, ...base }, version: 1, clip: null, clipError: "anchored by the page", label: "The save button" });
    await waitFor(() => document.querySelector(".composer-quote")?.textContent === "The save button", "the page's label");
    expect(ofType("state").at(-1)!.draft!.label).toBe("The save button");
  });

  it("refuses a press attestation near any of the shell's recent presses, not only the latest", async () => {
    let t = 70_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { frame, host } = await appView();
    const control = document.createElement("button");
    document.body.append(control);
    dispatchTrusted(control, new MouseEvent("pointerdown", { bubbles: true, clientX: 200, clientY: 300 }));
    t += 40;
    dispatchTrusted(control, new MouseEvent("pointerdown", { bubbles: true, clientX: 20, clientY: 40 }));
    control.remove();
    document.elementFromPoint = () => frame;
    Object.defineProperty(navigator, "userActivation", { value: { isActive: true }, configurable: true });
    frame.focus();
    t += 100;
    host.attestPointer({ pointer: 1, phase: "down", x: 200, y: 300 });
    expect(gesture!.frameGesture()).toBe(false);
    vi.restoreAllMocks();
  });
});

describe("app mode's visibility", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  const hideDocument = () => {
    Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
    document.dispatchEvent(new Event("visibilitychange"));
  };
  const clock = () => (globalThis as unknown as { claxTestClock: { advance(ms: number): number } }).claxTestClock;
  afterEach(() => {
    delete (document as unknown as { visibilityState?: unknown }).visibilityState;
    delete (window as unknown as { ClaxHost?: unknown }).ClaxHost;
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    document.documentElement.classList.remove("app");
    latest = 1;
  });
  const artifactTopic = `artifact:${ID}`;
  const holdsArtifact = async () => (await import("../stream")).pageStream().topics.includes(artifactTopic);
  /** Waits until the page's stream holds the artifact's topic, or (`held` false) does not. */
  const untilHeld = async (holds: boolean, what: string) => {
    const st = (await import("../stream")).pageStream();
    await waitFor(() => st.topics.includes(artifactTopic) === holds, what);
  };

  it("a hidden document does not stop the app's view following: a version 60 s later still lands", async () => {
    hideDocument();
    const { view, frame } = await appView();
    hello(frame, 1);
    await waitFor(() => FakeES.last, "stream");
    const { HIDDEN_MS } = await import("../stream");
    clock().advance(2 * HIDDEN_MS);
    await settle();
    expect(await holdsArtifact()).toBe(true);
    latest = 2;
    (await waitFor(() => FakeES.last, "stream still held")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    const next = await waitFor(() => { const f = view.root.querySelector<HTMLIFrameElement>("iframe.frame"); return f && f !== frame ? f : null; }, "the newer version's frame");
    hello(next, 2);
    expect(ofType("landed")).toEqual([{ type: "landed", version: 2 }]);
  });

  it("a browser's hidden view releases the artifact's topics after HIDDEN_MS (the rule app mode replaces)", async () => {
    history.replaceState(null, "", `/a/${ID}`);
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))));
    await waitFor(() => view.root.querySelector("iframe.frame"), "frame");
    await waitFor(() => FakeES.last, "stream");
    hideDocument();
    const { HIDDEN_MS } = await import("../stream");
    clock().advance(HIDDEN_MS + 1);
    await untilHeld(false, "released");
  });

  it("setVisible(false) releases the artifact's topics after HIDDEN_MS, and setVisible(true) takes them again with a refetch", async () => {
    const { host } = await appView();
    await waitFor(() => FakeES.last, "stream");
    const { HIDDEN_MS } = await import("../stream");
    host.setVisible(false);
    clock().advance(HIDDEN_MS + 1);
    await untilHeld(false, "released");
    const loads = () => requests.filter(r => r.url === `/api/artifacts/${ID}`).length;
    host.setVisible(true);
    await untilHeld(true, "taken again");
    const before = loads();
    FakeES.last!.emit("ready", {});
    await waitFor(() => loads() > before, "a refetch");
  });

  it("before init the document's visibility applies, and a setVisible before init holds from init on", async () => {
    hideDocument();
    const { host } = await appView({ init: false });
    await waitFor(() => FakeES.last, "stream");
    host.setVisible(false);
    host.init({ nonce: NONCE });
    const { HIDDEN_MS } = await import("../stream");
    clock().advance(HIDDEN_MS + 1);
    await untilHeld(false, "released");
  });

  it("with no animation frames at all, the address bar follows the page's fragment and the composer's status shows", async () => {
    const { frame, host } = await appView();
    vi.stubGlobal("requestAnimationFrame", () => 1);
    vi.stubGlobal("cancelAnimationFrame", () => {});
    hello(frame, 1);
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "#later" });
    await waitFor(() => location.hash === "#later", "the fragment in the address bar");
    host.enterCommentMode();
    gestureIn(frame);
    fromFrame(frame.contentWindow!, startOf(pick("p1", "Quarterly goals")));
    await waitFor(() => document.querySelector(".composer [role=status]")?.textContent === "Taking the screenshot…", "the composer's status");
  });
});
