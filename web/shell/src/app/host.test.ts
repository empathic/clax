import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { workerWith } from "../test/fake-worker";
import { MOUNT_TIMEOUT_MS } from "../test/timeouts";
import { FakeES, ID, artifact, artifactViewHooks, buttonNamed, fromFrame, gesture, gestureIn, mountView, page, pick, startOf, viewerPick, waitFor } from "../test/artifact-view";
import { view as questionView } from "../q/fixtures";
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
/** The hosts installed by the current test, uninstalled after it: a host
 * left installed would still hear the document's events in later tests. */
const installed: (() => void)[] = [];
afterEach(() => { for (const u of installed.splice(0)) u(); });
/** While set, the artifact's answers wait for it (each answers with the
 * version that was latest when it was asked). */
let held: Promise<void> | null = null;

async function appView(opts: { pinned?: number | null; channel?: boolean; init?: boolean; threads?: unknown[]; comments?: (url: string, init?: RequestInit) => Response | undefined; api?: (url: string) => Response | undefined; query?: string } = {}) {
  const query = opts.query ?? "?app=1";
  history.replaceState(null, "", opts.pinned ? `/a/${ID}/v/${opts.pinned}${query}` : `/a/${ID}${query}`);
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
    const own = opts.api?.(url);
    if (own) return own;
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
  let done = false;
  const remove = m.installHost(window, ctl);
  const uninstall = () => { if (!done) { done = true; remove(); } };
  installed.push(uninstall);
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
/** jsdom has no object URLs, which the composer's screenshot takes. */
function blobUrls() {
  const u = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
  if (u.createObjectURL) return;
  u.createObjectURL = () => "blob:clip";
  u.revokeObjectURL = () => {};
  onTestFinished(() => { delete u.createObjectURL; delete u.revokeObjectURL; });
}
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
    blobUrls();
    const { frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    gestureIn(frame);
    const p = pick("p1", "Quarterly goals");
    fromFrame(frame.contentWindow!, startOf(p));
    await waitFor(() => ofType("state").at(-1)?.draft, "a draft");
    const draft = { key: "p1", kind: "element", label: "“Quarterly goals”", quote: "Quarterly goals" };
    expect(ofType("state").at(-1)).toEqual({ type: "state", commenting: false, draft: { ...draft, text: "", clip: "pending" }, busy: false });
    const textarea = await waitFor(() => document.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    textarea.value = "bigger";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    expect(ofType("state").at(-1)!.draft).toEqual({ ...draft, text: "bigger", clip: "pending" });
    expect(ofType("draftClip")).toEqual([]);
    fromFrame(frame.contentWindow!, { ...p, clipPng: new Uint8Array([1, 2, 3]).buffer });
    expect(ofType("state").at(-1)!.draft).toEqual({ ...draft, text: "bigger", clip: "ready" });
    // The screenshot goes to the app once, as base64, under the composer's key.
    await waitFor(() => ofType("draftClip").length, "the clip");
    expect(ofType("draftClip")).toEqual([{ type: "draftClip", key: "p1", png: "AQID" }]);
    textarea.value = "bigger still";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(ofType("draftClip").length).toBe(1);
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
    expect(ofType("threads").at(-1)!.threads).toEqual([{ id: "t9", status: "open", summary: "make it blue", label: "“Quarterly goals”", sent: true, comments: 1, version: 1, clip: false }]);
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

  it("a post the composer queued and then dropped reaches the app as a notice, each time", async () => {
    const { ctl, frame } = await appView();
    hello(frame, 1);
    ctl.app!.dropped();
    ctl.app!.dropped();
    await waitFor(() => ofType("notice").length >= 2, "two notices");
    expect(ofType("notice").map(n => n.text)).toEqual([
      "Not sent: the comment changed before its screenshot was taken. Send it again.",
      "Not sent: the comment changed before its screenshot was taken. Send it again.",
    ]);
  });

  it("submitDraft({text}) sends the app composer's text on the pick's anchor and screenshot", async () => {
    blobUrls();
    const posted = thread("t9");
    const { frame, host } = await appView({
      comments: (url, init) => {
        if (init?.method !== "POST") return undefined;
        if (url === `/api/artifacts/${ID}/threads`) return new Response(JSON.stringify({ thread: posted }));
        if (url === `/api/artifacts/${ID}/threads/t9/send`) return new Response(JSON.stringify({ thread: { ...posted, sent_to_agent: true } }));
        return undefined;
      },
    });
    hello(frame, 1);
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipPng: new Uint8Array([1, 2, 3]).buffer });
    await waitFor(() => document.querySelector<HTMLFormElement>(".composer"), "composer");
    await waitFor(() => ofType("state").at(-1)?.draft?.clip === "ready", "the clip");
    host.submitDraft({ text: "make it blue" });
    await waitFor(() => ofType("sent").length, "sent");
    const body = requests.find(r => r.method === "POST")!.body as FormData;
    expect(body.get("body")).toBe("make it blue");
    expect(JSON.parse(body.get("anchor") as string)).toMatchObject({ kind: "element", quote: "Quarterly goals" });
    expect(body.get("clip")).toBeInstanceOf(Blob);
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

  it("offers a newer version (newer) without loading it; reloadLatest loads it in place and asks the bridge to restore scroll", async () => {
    const { ctl, view, frame, host } = await appView();
    hello(frame, 1);
    fromFrame(frame.contentWindow!, { type: "clax:scroll", file: "index.html", y: 1200 });
    // A report for another page than the one shown is not kept.
    fromFrame(frame.contentWindow!, { type: "clax:scroll", file: "other.html", y: 5 });
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ofType("newer").length, "the offer");
    expect(ofType("newer")).toEqual([{ type: "newer", version: 2 }]);
    await settle();
    expect(view.root.querySelector("iframe.frame")).toBe(frame);
    expect(ofType("landed")).toEqual([]);
    host.reloadLatest();
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
    expect(ofType("newer")).toHaveLength(1);
  });

  it("offers each newer version once, and again to a new init's channel", async () => {
    const { frame, host } = await appView({ init: false });
    hello(frame, 1);
    latest = 2;
    const es = await waitFor(() => FakeES.last, "stream");
    es.emit("version", { type: "version", artifact_id: ID, n: 2 });
    await settle();
    // Nothing is posted before init; init offers what is due.
    expect(raw).toEqual([]);
    host.init({ nonce: NONCE });
    expect(ofType("newer")).toEqual([{ type: "newer", version: 2 }]);
    latest = 3;
    es.emit("version", { type: "version", artifact_id: ID, n: 3 });
    await waitFor(() => ofType("newer").length === 2, "the second offer");
    es.emit("version", { type: "version", artifact_id: ID, n: 3 });
    await settle();
    expect(ofType("newer")).toEqual([{ type: "newer", version: 2 }, { type: "newer", version: 3 }]);
  });

  it("a version published while reloadLatest loads is offered, not loaded", async () => {
    const { ctl, view, frame, host } = await appView();
    hello(frame, 1);
    latest = 2;
    const es = await waitFor(() => FakeES.last, "stream");
    es.emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ofType("newer").length, "the offer");
    let release!: () => void;
    held = new Promise(r => { release = r; });
    host.reloadLatest();
    await waitFor(() => requests.filter(r => r.url === `/api/artifacts/${ID}`).length >= 2, "the reload's request");
    latest = 3;
    es.emit("version", { type: "version", artifact_id: ID, n: 3 });
    await waitFor(() => ctl.state.get().newer === 3, "newer 3");
    held = null;
    release();
    await waitFor(() => view.root.querySelector("iframe.frame")?.getAttribute("src") === `/c/${ID}/v/2/`, "version 2's frame");
    await settle();
    expect(view.root.querySelector("iframe.frame")?.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    expect(ofType("newer").map(m => m.version)).toEqual([2, 3]);
  });

  it("reloadLatest without a reported scroll asks for no restore", async () => {
    const { view, frame, host } = await appView();
    hello(frame, 1);
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ofType("newer").length, "the offer");
    host.reloadLatest();
    const next = await waitFor(() => { const f = view.root.querySelector<HTMLIFrameElement>("iframe.frame"); return f && f !== frame ? f : null; }, "the newer version's frame");
    const posted = framePosts(next);
    hello(next, 2);
    expect(posted.find(m => m.type === "clax:welcome")).toEqual({ type: "clax:welcome", mode: "view", app: true });
  });

  it("an offer leaves an open composer and its text in place", async () => {
    const { ctl, view, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipError: "none" });
    await waitFor(() => document.querySelector(".composer textarea"), "composer");
    host.setDraftText("half a thought");
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ofType("newer").length, "the offer");
    host.cancelDraft();
    await settle();
    expect(view.root.querySelector("iframe.frame")).toBe(frame);
    expect(ctl.state.get().newer).toBe(2);
    expect(ofType("landed")).toEqual([]);
  });

  it("a pinned view is offered nothing and never moves", async () => {
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
    expect(ofType("newer")).toEqual([]);
  });

  it("history: the page's fragments are the app's own history, and back() moves back through them in place", async () => {
    const { frame, host } = await appView();
    expect(ofType("history")).toEqual([{ type: "history", back: false }]);
    hello(frame, 1);
    const posted = framePosts(frame);
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "" });
    await settle();
    expect(ofType("history")).toHaveLength(1);
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "#/item/3" });
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: true });
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "#/item/4" });
    // An invalid fragment is no entry.
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "item/5" });
    host.back();
    expect(posted.filter(m => m.type === "clax:hash").at(-1)).toEqual({ type: "clax:hash", hash: "#/item/3" });
    expect(location.pathname + location.search + location.hash).toBe(`/a/${ID}?app=1#/item/3`);
    // The page reports the fragment it moved to: no new entry.
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "#/item/3" });
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: true });
    host.back();
    expect(posted.filter(m => m.type === "clax:hash").at(-1)).toEqual({ type: "clax:hash", hash: "" });
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: false });
    const sent = posted.length;
    host.back();
    expect(posted.length).toBe(sent);
  });

  it("history: the page's pushState routes are the app's own history too, and back() returns the page to the route before in place", async () => {
    const { frame, host } = await appView();
    hello(frame, 1);
    const posted = framePosts(frame);
    const routeBacks = () => posted.filter(m => m.type === "clax:back");
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "" });
    fromFrame(frame.contentWindow!, { type: "clax:route", depth: 1, hash: "" });
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: true });
    fromFrame(frame.contentWindow!, { type: "clax:route", depth: 2, hash: "" });
    // A fragment on a route is an entry of that route.
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "#laps" });
    // An invalid report is none.
    fromFrame(frame.contentWindow!, { type: "clax:route", depth: -1, hash: "" });
    fromFrame(frame.contentWindow!, { type: "clax:route", depth: 3, hash: "laps" });
    host.back();
    expect(routeBacks()).toEqual([]);
    expect(posted.filter(m => m.type === "clax:hash").at(-1)).toEqual({ type: "clax:hash", hash: "" });
    host.back();
    expect(routeBacks()).toEqual([{ type: "clax:back", depth: 1 }]);
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: true });
    // The page reports the route it returned to: no new entry.
    fromFrame(frame.contentWindow!, { type: "clax:route", depth: 1, hash: "" });
    // The page's own back to its first route: the app's history follows.
    fromFrame(frame.contentWindow!, { type: "clax:route", depth: 0, hash: "" });
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: false });
    const sent = posted.length;
    host.back();
    expect(posted.length).toBe(sent);
  });

  it("history: a page's routes go with its document; back() from another page reloads it", async () => {
    const { frame, host } = await appView();
    hello(frame, 1);
    fromFrame(frame.contentWindow!, { type: "clax:route", depth: 1, hash: "" });
    // The page's link to another page: a new document greets.
    hello(frame, 1, "about.html");
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: true });
    const posted = framePosts(frame);
    host.back();
    expect(posted.filter(m => m.type === "clax:back")).toEqual([]);
    expect(location.pathname).toBe(`/a/${ID}`);
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: false });
  });

  it("history: a version swap keeps it, and back() then moves the new version's page", async () => {
    const { view, frame, host } = await appView();
    hello(frame, 1);
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "#/a" });
    fromFrame(frame.contentWindow!, { type: "clax:hash", hash: "#/b" });
    latest = 2;
    (await waitFor(() => FakeES.last, "stream")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ofType("newer").length, "the offer");
    host.reloadLatest();
    const next = await waitFor(() => { const f = view.root.querySelector<HTMLIFrameElement>("iframe.frame"); return f && f !== frame ? f : null; }, "the newer version's frame");
    expect(next.getAttribute("src")).toBe(`/c/${ID}/v/2/#/b`);
    const posted = framePosts(next);
    hello(next, 2);
    fromFrame(next.contentWindow!, { type: "clax:hash", hash: "#/b" });
    expect(ofType("history").at(-1)).toEqual({ type: "history", back: true });
    host.back();
    expect(posted.filter(m => m.type === "clax:hash").at(-1)).toEqual({ type: "clax:hash", hash: "#/a" });
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
    expect(ofType("threads").at(-1)!.threads).toEqual([{ id: "t1", status: "open", summary: "make it blue", label: "“Quarterly goals”", sent: false, comments: 1, version: 1, clip: false }]);
    host.openThread("nope");
    expect(ctl.state.get().selected).toBeNull();
    host.openThread("t1");
    expect(ctl.state.get().selected).toBe("t1");
  });

  it("focusThread outlines the thread while the app's thread view is open, until it closes or comment mode ends", async () => {
    const { ctl, frame, host } = await appView({ threads: [thread("t1")] });
    const posted = framePosts(frame);
    hello(frame, 1);
    await waitFor(() => ctl.state.get().threads.length, "threads");
    const focus = () => posted.filter(m => m.type === "clax:focus").at(-1)?.id;
    await waitFor(() => posted.some(m => m.type === "clax:resolve-anchors"), "anchors sent");
    host.focusThread("t1");
    const handle = focus();
    expect(typeof handle).toBe("string");
    host.focusThread(null);
    expect(focus()).toBeNull();
    host.focusThread("nope");
    expect(focus()).toBeNull();
    host.focusThread("t1");
    expect(focus()).toBe(handle);
    host.enterCommentMode();
    await waitFor(() => posted.some(m => m.type === "clax:comment-mode" && m.on === true), "comment mode on");
    expect(focus()).toBe(handle);
    host.exitCommentMode();
    await waitFor(() => focus() === null, "the outline cleared as comment mode ends");
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

  it("counts an attested press on the frame as the finger's arrival until its release, and ATTEST_MS after it", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { frame, host, m } = await appView();
    hitTest(() => frame);
    fingerOnPage(frame);
    expect(gesture!.frameGesture()).toBe(false);
    host.attestPointer({ pointer: 1, phase: "down", x: 100, y: 100 });
    expect(gesture!.frameGesture()).toBe(true);
    expect(gesture!.frameGestureStrict()).toBe("ok");
    // Held (a hold, then a slow drag): still the finger's arrival.
    t += 5 * m.ATTEST_MS;
    expect(gesture!.frameGesture()).toBe(true);
    // The release, attested, counts for ATTEST_MS more.
    host.attestPointer({ pointer: 1, phase: "up", x: 100, y: 120 });
    expect(gesture!.frameGesture()).toBe(true);
    t += m.ATTEST_MS - 1;
    expect(gesture!.frameGesture()).toBe(true);
    t += 1;
    expect(gesture!.frameGesture()).toBe(false);
  });

  it("stops counting a press whose release never comes after HELD_MAX_MS", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { frame, host, m } = await appView();
    hitTest(() => frame);
    fingerOnPage(frame);
    host.attestPointer({ pointer: 1, phase: "down", x: 100, y: 100 });
    t += m.HELD_MAX_MS - 1;
    expect(gesture!.frameGesture()).toBe(true);
    t += 1;
    expect(gesture!.frameGesture()).toBe(false);
  });

  it("expires an unreleased press HELD_MAX_MS after its own press, whatever attests later", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { frame, host, m } = await appView();
    hitTest(() => frame);
    fingerOnPage(frame);
    // A press whose release never comes (an OS-cancelled touch not reported).
    host.attestPointer({ pointer: 1, phase: "down", x: 100, y: 100 });
    t += m.HELD_MAX_MS - 500;
    // Another finger's tap, released.
    host.attestPointer({ pointer: 2, phase: "down", x: 100, y: 100 });
    host.attestPointer({ pointer: 2, phase: "up", x: 100, y: 100 });
    expect(gesture!.hostArrival.pending()).toBe(true);
    t += 500;
    // The stale press is gone: nothing pending, and the release counts
    // ATTEST_MS, not HELD_MAX_MS.
    expect(gesture!.hostArrival.pending()).toBe(false);
    t += m.ATTEST_MS - 501;
    expect(gesture!.frameGesture()).toBe(true);
    t += 1;
    expect(gesture!.frameGesture()).toBe(false);
  });

  it("takes a cancel attestation as the end of a press that grants nothing", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { ctl, frame, host, m } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    host.attestPointer({ pointer: 1, phase: "down", x: 50, y: 60 });
    expect(gesture!.hostArrival.pending()).toBe(true);
    // After the viewer's shell input, a cancel counts as no arrival, and
    // ends the press, so nothing is pending.
    gesture!.noteShellInput();
    t += 1;
    host.attestPointer({ pointer: 1, phase: "cancel", x: 50, y: 60 });
    expect(gesture!.hostArrival.pending()).toBe(false);
    expect(gesture!.frameGesture()).toBe(false);
    // Its pick start is refused at once, with no wait.
    fromFrame(frame.contentWindow!, startOf(pick("cancelled", "Quarterly goals")));
    expect(ctl.state.get().draft).toBeNull();
    // A release reported after the cancel attests nothing either.
    host.attestPointer({ pointer: 1, phase: "up", x: 50, y: 60 });
    expect(gesture!.frameGesture()).toBe(false);
    // A cancel shortens no other window: a tap's release still counts ATTEST_MS.
    host.attestPointer({ pointer: 2, phase: "down", x: 50, y: 60 });
    host.attestPointer({ pointer: 2, phase: "up", x: 50, y: 60 });
    host.attestPointer({ pointer: 3, phase: "cancel", x: 50, y: 60 });
    t += m.ATTEST_MS - 1;
    expect(gesture!.frameGesture()).toBe(true);
  });

  it("takes a slow hold-then-drag's pick whose release is attested after the pick starts", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    host.attestPointer({ pointer: 1, phase: "down", x: 50, y: 60 });
    // A 600 ms hold and a 600 ms drag; the bridge's pick starts on touchend,
    // before the host's release attestation reaches the shell.
    t += 1_200;
    fromFrame(frame.contentWindow!, startOf(pick("slow", "Quarterly goals")));
    expect(ctl.state.get().draft?.pickId).toBe("slow");
    host.attestPointer({ pointer: 1, phase: "up", x: 210, y: 60 });
    expect(ctl.state.get().draft?.pickId).toBe("slow");
  });

  it("lets a pick start wait PICK_ATTEST_WAIT_MS for a pending release attestation, then takes or refuses it", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    const clock = (globalThis as unknown as { claxTestClock: { advance(ms: number): number } }).claxTestClock;
    // A held press made before the viewer's latest shell input no longer
    // counts on its own: its release, attested within the wait, does.
    host.attestPointer({ pointer: 1, phase: "down", x: 50, y: 60 });
    gesture!.noteShellInput();
    t += 1;
    fromFrame(frame.contentWindow!, startOf(pick("late", "Quarterly goals")));
    expect(ctl.state.get().draft).toBeNull();
    clock.advance(gesture!.PICK_ATTEST_WAIT_MS - 1);
    expect(ctl.state.get().draft).toBeNull();
    host.attestPointer({ pointer: 1, phase: "up", x: 50, y: 60 });
    expect(ctl.state.get().draft?.pickId).toBe("late");
  });

  it("refuses a waiting pick start when no release attestation comes in time", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    const clock = (globalThis as unknown as { claxTestClock: { advance(ms: number): number } }).claxTestClock;
    const refused: unknown[] = [];
    frame.contentWindow!.addEventListener("message", e => { if ((e.data as { type?: string })?.type === "clax:pick-refused") refused.push(e.data); });
    host.attestPointer({ pointer: 1, phase: "down", x: 50, y: 60 });
    gesture!.noteShellInput();
    t += 1;
    fromFrame(frame.contentWindow!, startOf(pick("stale", "Quarterly goals")));
    expect(refused).toEqual([]);
    clock.advance(gesture!.PICK_ATTEST_WAIT_MS);
    await waitFor(() => refused.length > 0 || null, "pick-refused");
    expect(ctl.state.get().draft).toBeNull();
    // The release coming after the wait takes nothing.
    host.attestPointer({ pointer: 1, phase: "up", x: 50, y: 60 });
    expect(ctl.state.get().draft).toBeNull();
  });

  it("starts no wait without a pending press: a forged or unpaired start is refused at once", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    hitTest(() => frame);
    fingerOnPage(frame);
    // An unpaired release attests nothing, and leaves nothing pending.
    host.attestPointer({ pointer: 9, phase: "up", x: 50, y: 60 });
    expect(gesture!.hostArrival.pending()).toBe(false);
    fromFrame(frame.contentWindow!, startOf(pick("forged", "Forged")));
    expect(ctl.state.get().draft).toBeNull();
    // A press attested later does not revive the refused start.
    host.attestPointer({ pointer: 2, phase: "down", x: 50, y: 60 });
    expect(ctl.state.get().draft).toBeNull();
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
    expect(toHost.map(m => m.type)).toEqual(["state", "threads", "ready", "history"]);
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
    installed.push(m.installHost(window, view.controller()));
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
    expect(ofType("state").at(-1)!.draft!.label).toBe("“Delete run”");
    // The viewer's own pick is not marked as the page's.
    expect(ofType("state").at(-1)!.draft!.byPage).toBeUndefined();
    expect(JSON.stringify(toHost)).not.toContain("div.run");
    // The shell's own composer says the same, not the selector.
    await waitFor(() => document.querySelector(".composer-quote")?.textContent === "“Delete run”", "the composer's words");
  });

  it("shows a pick's spaced text in place of its run-together quote, for its draft and the threads with that quote", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    gestureIn(frame);
    const row = { kind: "element", selector: "body > div", quote: "Mon5 km", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
    fromFrame(frame.contentWindow!, { type: "clax:pick-start", pickId: "m1", version: 1, anchor: row, desc: { tag: "div", role: null, name: null, text: "Mon 5 km" } });
    await waitFor(() => ofType("state").at(-1)?.draft, "a draft");
    expect(ofType("state").at(-1)!.draft!.label).toBe("“Mon 5 km”");
    await waitFor(() => document.querySelector(".composer-quote")?.textContent === "“Mon 5 km”", "the composer's words");
    ctl.changeThreads(ts => [...ts, thread("t5", "longer", { anchor: row }), thread("t6", "other", { anchor: { ...row, quote: "Tue6 km" } })] as typeof ts);
    await waitFor(() => ofType("threads").at(-1)?.threads.length === 2, "threads");
    expect(ofType("threads").at(-1)!.threads.map(t => t.label)).toEqual(["“Mon 5 km”", "“Tue6 km”"]);
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
    await waitFor(() => document.querySelector(".composer-quote")?.textContent === "A spot on the page", "the composer's words");
    expect(ofType("state").at(-1)!.draft).toMatchObject({ label: "A spot on the page", byPage: true });
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

  it("a hidden document does not stop the app's view following: a version 60 s later is still offered and lands", async () => {
    hideDocument();
    const { view, frame, host } = await appView();
    hello(frame, 1);
    await waitFor(() => FakeES.last, "stream");
    const { HIDDEN_MS } = await import("../stream");
    clock().advance(2 * HIDDEN_MS);
    await settle();
    expect(await holdsArtifact()).toBe(true);
    latest = 2;
    (await waitFor(() => FakeES.last, "stream still held")).emit("version", { type: "version", artifact_id: ID, n: 2 });
    await waitFor(() => ofType("newer").length, "the offer");
    host.reloadLatest();
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

describe("app mode's whole-app comment", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  afterEach(() => {
    delete (window as unknown as { ClaxHost?: unknown }).ClaxHost;
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    document.documentElement.classList.remove("app");
    latest = 1;
  });

  it("composeArtifact opens the composer on the whole app, with no screenshot, and Send posts an artifact anchor", async () => {
    const posted = thread("tw", "all of it", { anchor: { kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" } });
    const { ctl, frame, host } = await appView({
      comments: (url, init) => {
        if (init?.method !== "POST") return undefined;
        if (url === `/api/artifacts/${ID}/threads`) return new Response(JSON.stringify({ thread: posted }));
        if (url === `/api/artifacts/${ID}/threads/tw/send`) return new Response(JSON.stringify({ thread: { ...posted, sent_to_agent: true } }));
        return undefined;
      },
    });
    const framed = framePosts(frame);
    hello(frame, 1);
    host.composeArtifact();
    await waitFor(() => ofType("state").at(-1)?.draft, "a draft");
    expect(ofType("state").at(-1)!.draft).toMatchObject({ kind: "artifact", label: "The whole app", quote: null, text: "", clip: "none" });
    expect(ctl.state.get().draft!.anchor.kind).toBe("artifact");
    await waitFor(() => document.querySelector(".composer textarea"), "composer");
    // No pick, so nothing is asked of the page.
    expect(framed.filter(m => m.type === "clax:composer-ready")).toEqual([]);
    host.setDraftText("all of it");
    host.submitDraft();
    await waitFor(() => ofType("sent").length, "sent");
    const form = requests.find(r => r.method === "POST")!.body as FormData;
    expect(JSON.parse(form.get("anchor") as string)).toMatchObject({ kind: "artifact", selector: null, quote: null });
    expect(form.get("clip")).toBeNull();
  });

  it("composeArtifact keeps an open composer that holds typed text", async () => {
    const { ctl, frame, host } = await appView();
    hello(frame, 1);
    host.enterCommentMode();
    viewerPick(frame, { ...pick("p1", "Quarterly goals"), clipError: "none here" });
    await waitFor(() => document.querySelector(".composer textarea"), "composer");
    host.setDraftText("bigger");
    host.composeArtifact();
    expect(ctl.state.get().draft!.anchor.kind).toBe("element");
    host.setDraftText("");
    host.composeArtifact();
    expect(ctl.state.get().draft!.anchor.kind).toBe("artifact");
  });
});

describe("app mode's inspect view", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  afterEach(() => {
    delete (window as unknown as { ClaxHost?: unknown }).ClaxHost;
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    document.documentElement.classList.remove("app", "inspect");
    latest = 1;
  });
  const INSPECT = "?app=1&inspect=1";
  const MIGRATE = "?app=1&inspect=1&migrate=1";
  const shot = new Uint8Array([137, 80, 78, 71, 1, 2, 3]);
  const b64 = btoa(String.fromCharCode(...shot));
  const answer = (requestId: string, over: Record<string, unknown> = {}) => ({
    type: "clax:inspection", requestId, png: shot.slice().buffer, errors: [{ kind: "error", text: "boom" }], failed: ["https://cdn.test/a.png"], text: "# Groceries", title: "Groceries", height: 1200, ...over,
  });

  it("inspect asks the page and posts its inspection with the nonce", async () => {
    const { frame, host } = await appView({ query: INSPECT });
    expect(document.documentElement.classList.contains("inspect")).toBe(true);
    const framed = framePosts(frame);
    hello(frame, 1);
    host.inspect({ settleMs: 500, maxBytes: 4096 });
    const ask = framed.find(m => m.type === "clax:inspect")!;
    expect(ask).toEqual({ type: "clax:inspect", requestId: expect.any(String), settleMs: 500, maxBytes: 4096 });
    // Another request's answer, or one from elsewhere, is not taken.
    fromFrame(frame.contentWindow!, answer("other"));
    fromFrame(window, answer(ask.requestId as string));
    expect(ofType("inspection")).toEqual([]);
    fromFrame(frame.contentWindow!, answer(ask.requestId as string));
    expect(ofType("inspection")).toEqual([{ type: "inspection", png: b64, errors: [{ kind: "error", text: "boom" }], failed: ["https://cdn.test/a.png"], text: "# Groceries", title: "Groceries", height: 1200 }]);
    expect(raw.filter(r => r.includes("\"inspection\"")).every(r => JSON.parse(r).nonce === NONCE)).toBe(true);
    // Answered once.
    fromFrame(frame.contentWindow!, answer(ask.requestId as string));
    expect(ofType("inspection")).toHaveLength(1);
    // The page never hears the nonce.
    expect(JSON.stringify(framed)).not.toContain(NONCE);
  });

  it("takes the defaults for missing limits, and only the latest request's answer", async () => {
    const { frame, host } = await appView({ query: INSPECT });
    const framed = framePosts(frame);
    hello(frame, 1);
    host.inspect({});
    host.inspect({ settleMs: -5, maxBytes: "big" as never });
    const asks = framed.filter(m => m.type === "clax:inspect");
    expect(asks.map(a => [a.settleMs, a.maxBytes])).toEqual([[3000, 1048576], [0, 1048576]]);
    fromFrame(frame.contentWindow!, answer(asks[0].requestId as string));
    expect(ofType("inspection")).toEqual([]);
    fromFrame(frame.contentWindow!, { type: "clax:inspection", requestId: asks[1].requestId, error: "the screenshot took longer than 4000 ms" });
    expect(ofType("inspection")).toEqual([{ type: "inspection", error: "the screenshot took longer than 4000 ms" }]);
  });

  it("checks the shape of what the page answers", async () => {
    const { frame, host } = await appView({ query: INSPECT });
    const framed = framePosts(frame);
    hello(frame, 1);
    const ask = async (over: Record<string, unknown>, maxBytes = 1 << 20) => {
      host.inspect({ maxBytes });
      const id = framed.filter(m => m.type === "clax:inspect").at(-1)!.requestId as string;
      fromFrame(frame.contentWindow!, answer(id, over));
      return ofType("inspection").at(-1)!;
    };
    const many = Array.from({ length: 30 }, (_, i) => ({ kind: "console", text: `e${i}` }));
    const odd = await ask({ errors: [...many, { kind: "alert", text: "x" }, { kind: "error", text: 5 }, "nope"], failed: ["a", 3, "b"], text: "t".repeat(5000), title: 7, height: Number.NaN });
    expect(odd).toMatchObject({ failed: ["a", "b"], title: "", height: 0 });
    expect((odd as { errors: unknown[] }).errors).toHaveLength(20);
    expect((odd as { errors: { text: string }[] }).errors.at(-1)!.text).toBe("e29");
    expect((odd as { text: string }).text.length).toBeLessThanOrEqual(3072);
    expect(await ask({ png: "not bytes" })).toEqual({ type: "inspection", error: "the page sent no screenshot" });
    expect(await ask({}, 4)).toEqual({ type: "inspection", error: "the screenshot is over 4 bytes" });
  });

  it("answers an error before the page has greeted, and outside the inspect view", async () => {
    const { frame, host } = await appView({ query: INSPECT });
    const framed = framePosts(frame);
    host.inspect({});
    expect(ofType("inspection")).toEqual([{ type: "inspection", error: "the page has not loaded" }]);
    expect(framed.filter(m => m.type === "clax:inspect")).toEqual([]);
    const other = await appView();
    const otherFramed = framePosts(other.frame);
    hello(other.frame, 1);
    toHost = [];
    other.host.inspect({});
    expect(ofType("inspection")).toEqual([{ type: "inspection", error: "this view is not the inspect view" }]);
    expect(otherFramed.filter(m => m.type === "clax:inspect")).toEqual([]);
  });

  it("runTests asks the page to run the test files and posts the checked results", async () => {
    const { frame, host } = await appView({ query: INSPECT });
    const framed = framePosts(frame);
    hello(frame, 1);
    host.runTests({ files: ["tests/a.test.js", "tests/sub/b.test.js"], testMs: 2000 });
    const ask = framed.find(m => m.type === "clax:test")!;
    expect(ask).toEqual({ type: "clax:test", requestId: expect.any(String), files: ["tests/a.test.js", "tests/sub/b.test.js"], testMs: 2000, totalMs: 30000 });
    // Another request's answer, or one from elsewhere, is not taken.
    fromFrame(frame.contentWindow!, { type: "clax:tests", requestId: "other", passed: 1, failed: [], skipped: 0 });
    fromFrame(window, { type: "clax:tests", requestId: ask.requestId, passed: 1, failed: [], skipped: 0 });
    expect(ofType("tests")).toEqual([]);
    fromFrame(frame.contentWindow!, { type: "clax:tests", requestId: ask.requestId, passed: 2, failed: [{ name: "adds", error: "Expected: 1 to be 2" }, { name: 3 }, "x"], skipped: 1, extra: true });
    expect(ofType("tests")).toEqual([{ type: "tests", passed: 2, failed: [{ name: "adds", error: "Expected: 1 to be 2" }], skipped: 1 }]);
    expect(raw.filter(r => r.includes("\"tests\"")).every(r => JSON.parse(r).nonce === NONCE)).toBe(true);
    // Answered once; a bad shape is an error.
    fromFrame(frame.contentWindow!, { type: "clax:tests", requestId: ask.requestId, passed: 3, failed: [], skipped: 0 });
    expect(ofType("tests")).toHaveLength(1);
    host.runTests({ files: ["tests/a.test.js"] });
    const again = framed.filter(m => m.type === "clax:test").at(-1)!;
    fromFrame(frame.contentWindow!, { type: "clax:tests", requestId: again.requestId, passed: -1, failed: [], skipped: 0 });
    expect(ofType("tests").at(-1)).toEqual({ type: "tests", error: "the page sent no test results" });
  });

  it("runTests refuses paths outside tests/, before the page greets, and outside the inspect view", async () => {
    const { frame, host } = await appView({ query: INSPECT });
    const framed = framePosts(frame);
    host.runTests({ files: ["tests/a.test.js"] });
    expect(ofType("tests")).toEqual([{ type: "tests", error: "the page has not loaded" }]);
    hello(frame, 1);
    for (const files of [[], ["index.html"], ["tests/../x.test.js"], ["https://x.test/tests/a.test.js"], "tests/a.test.js"]) host.runTests({ files: files as string[] });
    expect(ofType("tests")).toHaveLength(6);
    expect(ofType("tests").slice(1).every(m => "error" in m && /^files is a list/.test(m.error as string))).toBe(true);
    expect(framed.filter(m => m.type === "clax:test")).toEqual([]);
    const other = await appView();
    hello(other.frame, 1);
    toHost = [];
    other.host.runTests({ files: ["tests/a.test.js"] });
    expect(ofType("tests")).toEqual([{ type: "tests", error: "this view is not the inspect view" }]);
  });

  it("runMigration asks the page to run the migration and posts its checked answer", async () => {
    const { frame, host } = await appView({ query: MIGRATE });
    const framed = framePosts(frame);
    expect(frame.name).toBe("clax-migrate");
    hello(frame, 1);
    host.runMigration({ file: "migrate.js", from: 2, to: 3 });
    const ask = framed.find(m => m.type === "clax:migrate")!;
    expect(ask).toEqual({ type: "clax:migrate", requestId: expect.any(String), file: "migrate.js", from: 2, to: 3, totalMs: 30000 });
    // Another request's answer, or one from elsewhere, is not taken.
    fromFrame(frame.contentWindow!, { type: "clax:migration", requestId: "other", ok: true });
    fromFrame(window, { type: "clax:migration", requestId: ask.requestId, ok: true });
    expect(ofType("migration")).toEqual([]);
    fromFrame(frame.contentWindow!, { type: "clax:migration", requestId: ask.requestId, ok: false, error: "x".repeat(900), extra: 1 });
    expect(ofType("migration")).toEqual([{ type: "migration", ok: false, error: "x".repeat(501) }]);
    expect(raw.filter(r => r.includes("\"migration\"")).every(r => JSON.parse(r).nonce === NONCE)).toBe(true);
    // Answered once.
    fromFrame(frame.contentWindow!, { type: "clax:migration", requestId: ask.requestId, ok: true });
    expect(ofType("migration")).toHaveLength(1);
    // Only the latest request's answer counts; a bad shape is an error.
    host.runMigration({ file: "migrate.js", from: 1, to: 2, totalMs: 500 });
    host.runMigration({ file: "lib/migrate.js", from: 1, to: 2, totalMs: 10 ** 9 });
    const [first, second] = framed.filter(m => m.type === "clax:migrate").slice(1);
    expect([first.totalMs, second.totalMs, second.file]).toEqual([500, 120000, "lib/migrate.js"]);
    fromFrame(frame.contentWindow!, { type: "clax:migration", requestId: first.requestId, ok: true });
    expect(ofType("migration")).toHaveLength(1);
    fromFrame(frame.contentWindow!, { type: "clax:migration", requestId: second.requestId, ok: "yes" });
    expect(ofType("migration").at(-1)).toEqual({ type: "migration", ok: false, error: "the page sent no migration result" });
    host.runMigration({ file: "migrate.js", from: 1, to: 2 });
    const last = framed.filter(m => m.type === "clax:migrate").at(-1)!;
    fromFrame(frame.contentWindow!, { type: "clax:migration", requestId: last.requestId, ok: true });
    expect(ofType("migration").at(-1)).toEqual({ type: "migration", ok: true });
  });

  it("a journaled migration's writes wait for the app's ack, and fail once the run has answered", async () => {
    const { ctl, frame, host } = await appView({ query: MIGRATE });
    const framed = framePosts(frame);
    hello(frame, 1);
    // A run without the journal (the update's check, on a draft) writes freely.
    host.runMigration({ file: "migrate.js", from: 1, to: 2 });
    expect(ctl.dbJournal).toBeNull();
    host.runMigration({ file: "migrate.js", from: 1, to: 2, journal: true });
    const journal = ctl.dbJournal!;
    const settled: string[] = [];
    const one = journal({ path: "data/users/me/profile", before: { v: 1 } }).then(() => settled.push("one"), () => settled.push("one refused"));
    const two = journal({ path: "a/2", before: null }).then(() => settled.push("two"), () => settled.push("two refused"));
    expect(ofType("journal")).toEqual([
      { type: "journal", seq: 1, path: "data/users/me/profile", before: { v: 1 } },
      { type: "journal", seq: 2, path: "a/2", before: null },
    ]);
    host.journalAck({ seq: 2, ok: false });
    host.journalAck({ seq: 1, ok: true });
    host.journalAck({ seq: 1, ok: true });
    await Promise.all([one, two]);
    expect(settled).toEqual(["two refused", "one"]);
    const three = journal({ path: "a/3", before: null });
    const ask = framed.filter(m => m.type === "clax:migrate").at(-1)!;
    fromFrame(frame.contentWindow!, { type: "clax:migration", requestId: ask.requestId, ok: false, error: "boom" });
    await expect(three).rejects.toMatchObject({ code: "unavailable" });
    await expect(journal({ path: "a/4", before: null })).rejects.toMatchObject({ code: "unavailable" });
    // Nor in a later run without it: the document stays journaled.
    host.runMigration({ file: "migrate.js", from: 1, to: 2 });
    expect(ctl.dbJournal).toBe(journal);
    await expect(journal({ path: "a/5", before: null })).rejects.toMatchObject({ code: "unavailable" });
  });

  it("restoreJournal puts the entries back in reverse order, only in the migration view", async () => {
    const docs = (url: string) => (url.includes("/docs/") ? new Response("{}", { status: url.includes("gone") ? 404 : 200 }) : undefined);
    const { frame, host } = await appView({ query: MIGRATE, api: docs });
    hello(frame, 1);
    requests = [];
    host.restoreJournal({ entries: [{ path: "data/users/me/profile", before: { v: 1 } }, { path: "a/new", before: null }, { path: "a/gone", before: null }] });
    await expect.poll(() => ofType("journalRestored")).toEqual([{ type: "journalRestored", ok: true }]);
    expect(requests.filter(r => r.url.includes("/docs/")).map(r => [r.method, r.url, r.body])).toEqual([
      ["DELETE", `/api/artifacts/${ID}/docs/a/gone?lww=true`, null],
      ["DELETE", `/api/artifacts/${ID}/docs/a/new?lww=true`, null],
      ["PUT", `/api/artifacts/${ID}/docs/data/users/me/profile`, { data: { v: 1 }, lww: true }],
    ]);
    host.restoreJournal({ entries: [{ path: "../x", before: null }] });
    await expect.poll(() => ofType("journalRestored")).toHaveLength(2);
    expect(ofType("journalRestored")[1]).toMatchObject({ ok: false });
    host.restoreJournal({ entries: "x" } as never);
    expect(ofType("journalRestored")[2]).toMatchObject({ ok: false, error: expect.stringMatching(/^entries is a list/) });
    const other = await appView({ query: INSPECT });
    hello(other.frame, 1);
    toHost = [];
    other.host.restoreJournal({ entries: [] });
    expect(ofType("journalRestored")).toEqual([{ type: "journalRestored", ok: false, error: "this view is not the migration view" }]);
  });

  it("runMigration refuses a bad order, before the page greets, and outside the migration view", async () => {
    const { frame, host } = await appView({ query: MIGRATE });
    const framed = framePosts(frame);
    host.runMigration({ file: "migrate.js", from: 1, to: 2 });
    expect(ofType("migration")).toEqual([{ type: "migration", ok: false, error: "the page has not loaded" }]);
    hello(frame, 1);
    const bad = [
      { file: "../migrate.js", from: 1, to: 2 },
      { file: "https://x.test/migrate.js", from: 1, to: 2 },
      { file: "migrate.ts", from: 1, to: 2 },
      { file: "a/./migrate.js", from: 1, to: 2 },
      { file: "migrate.js", from: 0, to: 2 },
      { file: "migrate.js", from: 1, to: 1.5 },
      { file: "migrate.js", from: "1", to: 2 },
      undefined,
    ];
    for (const p of bad) host.runMigration(p as never);
    expect(ofType("migration")).toHaveLength(1 + bad.length);
    expect(ofType("migration").slice(1).every(m => !m.ok && /^file is a path/.test(m.error))).toBe(true);
    expect(framed.filter(m => m.type === "clax:migrate")).toEqual([]);
    for (const query of [INSPECT, undefined]) {
      const other = await appView({ query });
      hello(other.frame, 1);
      toHost = [];
      other.host.runMigration({ file: "migrate.js", from: 1, to: 2 });
      expect(ofType("migration")).toEqual([{ type: "migration", ok: false, error: "this view is not the migration view" }]);
    }
  });

  it("shows no comment UI: comment mode, a whole-app composer and a page's composer stay closed", async () => {
    const { ctl, frame, host } = await appView({ query: INSPECT });
    hello(frame, 1);
    host.enterCommentMode();
    expect(ctl.state.get().commenting).toBe(false);
    host.composeArtifact();
    expect(ctl.state.get().draft).toBeNull();
    ctl.commentsUi.openComposer({ anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }, version: 1, clip: null });
    expect(ctl.state.get().draft).toBeNull();
  });

  it("answers every consent prompt Don't allow without showing it", async () => {
    const { ctl, frame } = await appView({ query: INSPECT });
    hello(frame, 1);
    const prompt = (ctl as unknown as { prompt(p: unknown): Promise<string> }).prompt;
    await expect(prompt({ title: "Let “A” read “B”'s data?", body: "…", allow: "Allow", deny: "Don't allow" })).resolves.toBe("deny");
    expect(ctl.state.get().ask).toBeNull();
    expect(document.querySelector(".prompt-backdrop")).toBeNull();
  });

  it("follows no events, and drops nothing else's stream once uninstalled", async () => {
    const { uninstall } = await appView({ query: INSPECT });
    const st = (await import("../stream")).pageStream();
    await waitFor(() => Object.hasOwn(st, "watch"), "the stream stood down");
    await settle();
    expect(st.topics).toEqual([]);
    expect(st.watching).toEqual([]);
    st.watch(["gallery"], () => {});
    expect(st.watching).toEqual([]);
    uninstall();
    expect(Object.hasOwn(st, "watch")).toBe(false);
  });

  it("keeps inspect=1 only with app=1, and carries it from page to page", async () => {
    const { appQuery, inspectQuery, migrateQuery } = await import("../route");
    expect(appQuery("?app=1&inspect=1&x=2")).toBe("?app=1&inspect=1");
    expect(appQuery("?inspect=1")).toBe("");
    expect(inspectQuery("?inspect=1")).toBe(false);
    expect(inspectQuery("?app=1")).toBe(false);
    expect(inspectQuery("?inspect=1&app=1")).toBe(true);
    expect(appQuery("?migrate=1&app=1&inspect=1&x=2")).toBe("?app=1&inspect=1&migrate=1");
    expect(appQuery("?app=1&migrate=1")).toBe("?app=1");
    expect(inspectQuery("?app=1&inspect=1&migrate=1")).toBe(true);
    expect(migrateQuery("?app=1&inspect=1&migrate=1")).toBe(true);
    expect(migrateQuery("?app=1&inspect=1")).toBe(false);
  });
});

describe("app mode's questions", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  afterEach(() => {
    delete (window as unknown as { ClaxHost?: unknown }).ClaxHost;
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    document.documentElement.classList.remove("app", "inspect");
    latest = 1;
  });
  const sheet = () => document.querySelector<HTMLElement>(".app-questions");
  /** Not the owner's browser: the question module never loads, so the test alone tells the host of questions. */
  const notOwner = (url: string) => (url === "/api/token" ? new Response("{}", { status: 403 }) : undefined);
  /** The question module's word: the cards about the app, open and closed moments ago, by question ID. */
  const tell = (open: string[], closed: string[] = []) => document.dispatchEvent(new CustomEvent("clax-questions", { detail: { open, closed } }));
  /** Mounts question `id`'s card in the sheet's slot, as the question module does after it has told the change. */
  const card = (id: string) => {
    const c = document.createElement("article");
    c.className = "qcard";
    c.dataset.question = id;
    c.tabIndex = -1;
    c.innerHTML = `<label class="opt other"><input type="text"></label><textarea></textarea>`;
    sheet()!.querySelector(".questions-slot")!.append(c);
    return c;
  };
  const cardOf = (id: string) => sheet()!.querySelector<HTMLElement>(`.qcard[data-question="${id}"]`)!;
  /** Lets mutation observers run. */
  const observed = () => new Promise(r => setTimeout(r, 0));

  it("docks the open questions about the app in a sheet, posts questions {open, showing} as they change, and openQuestions shows them", async () => {
    const q = questionView({ id: "Q1" });
    const { frame, host } = await appView({
      api: url => {
        if (url === "/api/token") return new Response(JSON.stringify({ token: "t0ken" }));
        if (url.startsWith("/api/questions")) return new Response(JSON.stringify({ questions: [q], open: 1 }));
        if (url.startsWith("/api/inbox")) return new Response(JSON.stringify({ unread: 0, questions: [q], latest: [] }));
        return undefined;
      },
    });
    hello(frame, 1);
    expect(sheet()!.hidden).toBe(true);
    // The question module loads once the view knows this is the owner's browser, and its feed goes live.
    const w = await workerWith("questions");
    w.live(["questions", "inbox"]);
    const shown = await waitFor(() => sheet()!.querySelector<HTMLElement>(".qcard[data-question='Q1']"), "the question's card in the sheet");
    expect(document.querySelector(".sidebar .qcard")).toBeNull();
    await waitFor(() => ofType("questions").length, "questions posted");
    expect(ofType("questions")).toEqual([{ type: "questions", open: 1, showing: false }]);
    // It shows only when the app asks.
    expect(sheet()!.hidden).toBe(true);
    host.openQuestions({ question: "Q1" });
    expect(sheet()!.hidden).toBe(false);
    expect(document.activeElement).toBe(shown);
    expect(ofType("questions").at(-1)).toEqual({ type: "questions", open: 1, showing: true });
    // Answered (in another browser, say): none left open; the card says so until it leaves.
    w.emit("questions", "question", { type: "question", question: { ...q, status: "answered", answered_via: "shell", answers: [] } });
    await waitFor(() => ofType("questions").at(-1)?.open === 0, "the count posted again");
    expect(ofType("questions").at(-1)).toEqual({ type: "questions", open: 0, showing: true });
    expect(sheet()!.hidden).toBe(false);
    expect(raw.every(m => JSON.parse(m).nonce === NONCE)).toBe(true);
  });

  it("openQuestions({question}) shows the sheet when that question arrives, however late, and focuses its card once it is mounted", async () => {
    let t = 10_000;
    vi.spyOn(performance, "now").mockImplementation(() => t);
    const { host } = await appView({ api: notOwner });
    tell(["Q0"]);
    card("Q0");
    host.openQuestions({ question: "Q1" });
    // Another question's card is there: the sheet waits for this one.
    expect(sheet()!.hidden).toBe(true);
    t += 60_000;
    // The module tells the change first, then mounts the card.
    tell(["Q0", "Q1"]);
    expect(sheet()!.hidden).toBe(false);
    const c = card("Q1");
    await observed();
    expect(document.activeElement).toBe(c);
  });

  it("keeps a waiting openQuestions({question}) across a change that leaves this app no card", async () => {
    const { host } = await appView({ api: notOwner });
    host.openQuestions({ question: "X" });
    // Another app's question changed the feed before it heard X: nothing about this app.
    tell([]);
    expect(sheet()!.hidden).toBe(true);
    tell(["X"]);
    expect(sheet()!.hidden).toBe(false);
    card("X");
    await observed();
    expect(document.activeElement).toBe(cardOf("X"));
  });

  it("drops a waiting openQuestions({question}) on closeQuestions and when that question closes", async () => {
    const { host } = await appView({ api: notOwner });
    host.openQuestions({ question: "Q1" });
    host.closeQuestions();
    tell(["Q1"]);
    card("Q1");
    expect(sheet()!.hidden).toBe(true);
    tell([]);
    host.openQuestions({ question: "Q2" });
    // Answered elsewhere before it ever showed here.
    tell([], ["Q2"]);
    card("Q2");
    tell(["Q1"], ["Q2"]);
    expect(sheet()!.hidden).toBe(true);
    // A question that is already closed is not waited for.
    host.openQuestions({ question: "Q2" });
    tell(["Q1", "Q3"], ["Q2"]);
    expect(sheet()!.hidden).toBe(true);
  });

  it("openQuestions() shows what is there and waits for nothing", async () => {
    const { host } = await appView({ api: notOwner });
    host.openQuestions();
    tell(["Q1"]);
    card("Q1");
    expect(sheet()!.hidden).toBe(true);
    host.openQuestions();
    expect(sheet()!.hidden).toBe(false);
    expect(document.activeElement).toBe(cardOf("Q1"));
  });

  it("closes the sheet on Close, Escape, closeQuestions, and once no card is left, posting showing each time", async () => {
    const { host } = await appView({ api: notOwner });
    tell(["Q1"]);
    const c = card("Q1");
    host.openQuestions();
    sheet()!.querySelector<HTMLButtonElement>("button.app-questions-close")!.click();
    expect(sheet()!.hidden).toBe(true);
    host.openQuestions();
    c.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(sheet()!.hidden).toBe(true);
    host.openQuestions();
    host.closeQuestions();
    expect(sheet()!.hidden).toBe(true);
    host.openQuestions();
    c.remove();
    tell([]);
    expect(sheet()!.hidden).toBe(true);
    // A card that comes later, unasked, does not open it.
    tell(["Q2"]);
    card("Q2");
    expect(sheet()!.hidden).toBe(true);
    expect(ofType("questions").map(m => [m.open, m.showing])).toEqual([
      [1, false], [1, true], [1, false], [1, true], [1, false], [1, true], [1, false], [1, true], [0, false], [1, false],
    ]);
  });

  it("does not take focus from a field of the sheet when another question is asked for", async () => {
    const { host } = await appView({ api: notOwner });
    tell(["Q1"]);
    card("Q1");
    host.openQuestions();
    const field = cardOf("Q1").querySelector("input")!;
    field.focus();
    host.openQuestions({ question: "Q2" });
    tell(["Q1", "Q2"]);
    card("Q2");
    await observed();
    expect(document.activeElement).toBe(field);
  });

  it("posts the change only when it changes, and not before init, then the latest at init", async () => {
    const { host } = await appView({ api: notOwner, init: false });
    tell(["Q1", "Q2"]);
    expect(toHost).toEqual([]);
    host.init({ nonce: NONCE });
    expect(ofType("questions")).toEqual([{ type: "questions", open: 2, showing: false }]);
    tell(["Q1", "Q2"], ["Q3"]);
    tell(["Q1"], ["Q2"]);
    tell(["Q3"], ["Q2"]);
    tell([]);
    expect(ofType("questions").map(m => m.open)).toEqual([2, 1, 0]);
  });

  it("posts nothing while the question module has told nothing", async () => {
    const { host } = await appView({ api: notOwner });
    host.openQuestions();
    host.closeQuestions();
    expect(ofType("questions")).toEqual([]);
  });

  it("setDraftText fills the focused text field of a question in the sheet, before an open composer", async () => {
    const { ctl, frame, host } = await appView({ api: notOwner });
    hello(frame, 1);
    host.composeArtifact();
    const composer = await waitFor(() => document.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    tell(["Q1"]);
    const c = card("Q1");
    host.openQuestions();
    const other = c.querySelector<HTMLInputElement>("input")!;
    const typed: string[] = [];
    other.addEventListener("input", () => typed.push(other.value));
    other.focus();
    host.setDraftText("a green one");
    expect(other.value).toBe("a green one");
    expect(typed).toEqual(["a green one"]);
    expect(composer.value).toBe("");
    const area = c.querySelector("textarea")!;
    area.focus();
    host.setDraftText("no notes");
    expect(area.value).toBe("no notes");
    // Focus out of the sheet: the composer's text, as before.
    area.blur();
    host.setDraftText("for the composer");
    expect(composer.value).toBe("for the composer");
    expect(ctl.composerText).toBe("for the composer");
    // A field of a closed sheet is not filled.
    host.closeQuestions();
    other.focus();
    host.setDraftText("hidden");
    expect(other.value).toBe("a green one");
  });

  it("has no sheet in the inspect view, and openQuestions there does nothing", async () => {
    const { host } = await appView({ api: notOwner, query: "?app=1&inspect=1" });
    expect(sheet()).toBeNull();
    host.openQuestions({ question: "Q1" });
    host.closeQuestions();
    tell(["Q1"]);
    expect(ofType("questions")).toEqual([]);
  });

  it("ignores a malformed question ID or change", async () => {
    const { host } = await appView({ api: notOwner });
    tell(["Q1"]);
    card("Q1");
    for (const bad of [{ question: 5 }, { question: "" }, { question: "x".repeat(129) }]) host.openQuestions(bad as never);
    expect(sheet()!.hidden).toBe(true);
    document.dispatchEvent(new CustomEvent("clax-questions", { detail: { open: [1], closed: [] } }));
    document.dispatchEvent(new CustomEvent("clax-questions", { detail: { open: 2 } }));
    expect(ofType("questions")).toEqual([{ type: "questions", open: 1, showing: false }]);
  });

  it("uninstall removes the sheet and stops posting", async () => {
    const { uninstall } = await appView({ api: notOwner });
    expect(sheet()).not.toBeNull();
    uninstall();
    expect(sheet()).toBeNull();
    tell(["Q1"]);
    expect(ofType("questions")).toEqual([]);
  });
});

describe("app mode's device link", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  afterEach(() => {
    delete (window as unknown as { ClaxHost?: unknown }).ClaxHost;
    delete (window as unknown as { claxHost?: unknown }).claxHost;
    document.documentElement.classList.remove("app", "inspect");
    latest = 1;
  });

  it("posts the page's device calls under the nonce, those before init at init, and resolves the app's answers", async () => {
    const { deviceLink } = await import("../caps/device-link");
    const { host } = await appView({ init: false });
    const early = deviceLink.call!("location", "current", []);
    expect(raw).toEqual([]);
    host.init({ nonce: NONCE });
    const [first] = ofType("device");
    expect(first).toEqual({ type: "device", id: expect.any(String), cap: "location", method: "current", args: [] });
    // A random prefix per load: an earlier load's answer cannot resolve this call.
    expect(first.id).toMatch(/^[0-9a-f]{12}-1$/);
    host.deviceResult({ id: "d1", ok: true, value: "stale" });
    const later = deviceLink.call!("share", "text", ["hi"]);
    const second = ofType("device")[1];
    expect(second).toMatchObject({ cap: "share", method: "text", args: ["hi"] });
    host.deviceResult({ id: "d-unknown", ok: true, value: 1 });
    host.deviceResult({ id: first.id, ok: true, value: { lat: 1, lon: 2, accuracy: 3 } });
    host.deviceResult({ id: second.id, ok: false, error: { code: "cancelled", message: "the person closed the sheet" } });
    await expect(early).resolves.toEqual({ lat: 1, lon: 2, accuracy: 3 });
    await expect(later).rejects.toMatchObject({ code: "cancelled" });
  });

  it("passes the app's watch news to the device listeners", async () => {
    const { deviceLink } = await import("../caps/device-link");
    const { host } = await appView();
    const heard: unknown[] = [];
    const f = (e: unknown) => heard.push(e);
    deviceLink.events.add(f);
    try {
      host.deviceEvent({ cap: "location", watch: "w1", kind: "update", value: { lat: 1 } });
      host.deviceEvent({ cap: "location", watch: "w1", kind: "end", error: { code: "stopped", message: "left" } });
      expect(heard).toEqual([
        { cap: "location", watch: "w1", kind: "update", value: { lat: 1 } },
        { cap: "location", watch: "w1", kind: "end", error: { code: "stopped", message: "left" } },
      ]);
    } finally {
      deviceLink.events.delete(f);
    }
  });

  it("asks the app once whether the view is its home app, and a failed answer is no", async () => {
    const { deviceLink } = await import("../caps/device-link");
    const { host, uninstall } = await appView();
    const asked = deviceLink.home!();
    const again = deviceLink.home!();
    const [q] = ofType("device");
    expect(ofType("device")).toHaveLength(1);
    expect(q).toMatchObject({ cap: "home", method: "available", args: [] });
    host.deviceResult({ id: q.id, ok: true, value: true });
    await expect(asked).resolves.toBe(true);
    await expect(again).resolves.toBe(true);
    uninstall();
    expect(deviceLink.home).toBeUndefined();
    const second = await appView();
    const refused = deviceLink.home!();
    second.host.deviceResult({ id: ofType("device").at(-1)!.id, ok: false, error: { code: "refused", message: "no" } });
    await expect(refused).resolves.toBe(false);
  });

  it("links nothing in the inspect view, and on uninstall unlinks and rejects the calls still waiting", async () => {
    const { deviceLink } = await import("../caps/device-link");
    const { uninstall } = await appView();
    const waiting = deviceLink.call!("camera", "take", []);
    uninstall();
    expect(deviceLink.call).toBeNull();
    await expect(waiting).rejects.toMatchObject({ code: "unavailable" });
    await appView({ query: "?app=1&inspect=1" });
    expect(deviceLink.call).toBeNull();
  });

  it("openRoute moves the shell's fragment, refusing anything but a short #fragment", async () => {
    const { host } = await appView();
    host.openRoute("#/plants/3");
    expect(location.hash).toBe("#/plants/3");
    host.openRoute("javascript:alert(1)");
    host.openRoute(`#${"x".repeat(600)}`);
    expect(location.hash).toBe("#/plants/3");
  });
});
