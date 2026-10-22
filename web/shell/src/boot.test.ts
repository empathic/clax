import { readFileSync } from "node:fs";
import { FakeWorker } from "./test/fake-worker";
import { dispatchTrusted } from "../../bridge/test/trusted";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SKELETON_HTML } from "./view/skeleton";
import { MOUNT_TIMEOUT_MS, WAIT_MS } from "./test/timeouts";

const ID = "7q3k9mzx2b4t";
const loaded = { artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "x", current_version: 2, pinned: false }, versions: [{ artifact_id: ID, n: 2, label: null, created_at: "x", files: {} }] };
const viewer = { public_id: "u_0123456789abcdef012345", display_name: null, created_at: "x" };
/** The site hosts of the test's location (jsdom's `http://localhost:3000`). */
const SITE_HOSTS = { domain: "localhost", scheme: "http" as const, port: Number(location.port) || null };
const boot = () => ({ v: 1 as const, artifact: loaded, threads: [], viewer, site: SITE_HOSTS });
const EARLY = /<script id="clax-early">([\s\S]*?)<\/script>/.exec(readFileSync(join(__dirname, "../artifact.html"), "utf8"))![1];
/** The artifact's site origin, where its frame's documents post from. */
const SITE = `http://${ID}.localhost${location.port ? `:${location.port}` : ""}`;
/** The served frame's address: the site's handshake for version 2's index. */
const START = `${SITE}/_clax/session/start?next=%2F_clax%2Fv%2F2%2F`;
const ALLOW = "clipboard-write; fullscreen; storage-access";
const SERVED = `<iframe class="frame" title="artifact content" src="${START}" allow="${ALLOW}"></iframe>`;
const HELLO = { type: "clax:hello", artifact: ID, version: 2, file: "index.html" };
const CODE = `${SITE}/_clax/session?code=c1&next=%2F_clax%2Fv%2F2%2F`;


async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}

const fromFrame = (win: Window, data: unknown, origin = SITE) => dispatchTrusted(window, new MessageEvent("message", { data, origin, source: win }));
const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

/** The page as the daemon sends it, with `frame` in the stage. */
function served(frame: string): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = `<div class="page">${SKELETON_HTML.replace("<!--clax:frame-->", frame)}</div>`;
  document.body.append(root);
  return root;
}

/** Records what the shell posts to `frame`. */
function posts(frame: HTMLIFrameElement): { type: string; id?: string }[] {
  const posted: { type: string; id?: string }[] = [];
  frame.contentWindow!.postMessage = ((m: { type: string }) => { posted.push(m); }) as Window["postMessage"];
  return posted;
}

function stubFetch() {
  const f = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
    const url = String(input);
    if (url === "/api/token") return new Response(JSON.stringify({ token: "tk" }));
    if (url.endsWith("/site-code")) return new Response(JSON.stringify({ src: CODE }));
    if (url.includes("/threads")) return new Response(JSON.stringify({ threads: [], next_cursor: null }));
    if (url === "/api/viewers/me") return new Response(JSON.stringify({ viewer }));
    return new Response(JSON.stringify(loaded));
  });
  vi.stubGlobal("fetch", f);
  vi.stubGlobal("SharedWorker", FakeWorker);
  return f;
}

const mounted: { unmount(): void }[] = [];
/** Every gesture module instance a mount loaded: each watches the document until its `unwatchShell` runs. */
const gestureModules = new Set<typeof import("./caps/gesture")>();

async function mountServed(root: HTMLElement, b: ReturnType<typeof boot> | null) {
  const { mountArtifactView } = await import("./artifact");
  const { takeEarly } = await import("./view/boot");
  gestureModules.add(await import("./caps/gesture"));
  const view = mountArtifactView(root, { id: ID, pinnedVersion: null }, { boot: b, early: () => takeEarly() });
  mounted.push(view);
  return view;
}

/** Whether the gate let anything through: a welcome, or a capability answer. */
const answered = (posted: { type: string }[]) => posted.filter(m => m.type === "clax:welcome" || m.type === "clax:use-result" || m.type === "clax:call-result");

describe("the first load from the daemon's HTML", { timeout: MOUNT_TIMEOUT_MS }, () => {
  beforeEach(async () => { vi.resetModules(); (await import("./threads")).forgetViewer(); history.replaceState(null, "", `/a/${ID}`); });
  afterEach(() => {
    for (const v of mounted.splice(0)) v.unmount();
    for (const g of gestureModules) g.unwatchShell();
    gestureModules.clear();
    (window as { __claxEarly?: { take(): unknown } }).__claxEarly?.take();
    vi.unstubAllGlobals();
    sessionStorage.clear();
    delete (window as { __claxEarly?: unknown }).__claxEarly;
    document.body.replaceChildren();
    history.replaceState(null, "", "/");
  });

  it("replays a hello and a capability request that arrived before the shell mounted, and never fetches the artifact", async () => {
    const fetched = stubFetch();
    new Function(EARLY)();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    fromFrame(frame.contentWindow!, HELLO);
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "early", name: "permissions" });
    await mountServed(root, boot());
    expect(root.querySelector("iframe")).toBe(frame);
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
    await waitFor(() => posted.some(m => m.type === "clax:use-result" && m.id === "early"), "the early request's answer");
    expect(posted.findIndex(m => m.type === "clax:welcome")).toBeLessThan(posted.findIndex(m => m.type === "clax:use-result"));
    const urls = fetched.mock.calls.map(c => String(c[0]));
    expect(urls).not.toContain(`/api/artifacts/${ID}`);
    expect(urls.some(u => u.includes("/threads"))).toBe(false);
    expect(urls).not.toContain("/api/viewers/me");
    expect(root.querySelector(".topbar h1")!.textContent).toBe("T");
  });

  it("replays the served frame's handshake state, asks a code for it and sends the frame to the code", async () => {
    const fetched = stubFetch();
    new Function(EARLY)();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    fromFrame(frame.contentWindow!, { type: "clax:site-state", state: "s1", next: "/_clax/v/2/" });
    // Where the frame is sent: in place (jsdom does not follow it).
    const { FrameHost } = await import("./view/frame-host");
    const go = vi.spyOn(FrameHost.prototype, "go");
    await mountServed(root, boot());
    await waitFor(() => go.mock.calls.length, "the frame sent to its code");
    expect(go.mock.calls).toEqual([[CODE]]);
    expect(root.querySelector("iframe")).toBe(frame);
    const call = fetched.mock.calls.find(c => String(c[0]).endsWith("/site-code"))!;
    expect(String(call[0])).toBe(`/api/artifacts/${ID}/site-code`);
    expect(JSON.parse(String((call[1] as RequestInit).body))).toEqual({ next: "/_clax/v/2/", partitioned: true, state: "s1" });
  });

  it("adopting the served frame opens nothing: a request before its hello is not answered", async () => {
    stubFetch();
    new Function(EARLY)();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "too-soon", name: "permissions" });
    await mountServed(root, boot());
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "still-too-soon", name: "permissions" });
    await sleep(30);
    expect(answered(posted)).toEqual([]);
    fromFrame(frame.contentWindow!, HELLO);
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
    expect(posted.some(m => m.type === "clax:use-result")).toBe(false);
  });

  it("counts the served frame's early load, so a later document that never greets closes the gate", async () => {
    stubFetch();
    new Function(EARLY)();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    fromFrame(frame.contentWindow!, HELLO);
    frame.dispatchEvent(new Event("load"));
    await mountServed(root, boot());
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
    // The frame navigates to a document without the bridge: its load, with no
    // hello since the previous one, closes the gate.
    frame.dispatchEvent(new Event("load"));
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "after", name: "permissions" });
    await sleep(30);
    expect(posted.some(m => m.type === "clax:use-result")).toBe(false);
  });

  it("ignores what other windows and origins posted before the shell mounted", async () => {
    stubFetch();
    new Function(EARLY)();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    const other = document.createElement("iframe");
    document.body.append(other);
    fromFrame(other.contentWindow!, HELLO);
    fromFrame(window, HELLO);
    fromFrame(frame.contentWindow!, HELLO, "null");
    fromFrame(frame.contentWindow!, HELLO, `http://9zzzzzzzzzzz.localhost${location.port ? `:${location.port}` : ""}`);
    await mountServed(root, boot());
    await sleep(30);
    expect(answered(posted)).toEqual([]);
  });

  it("keeps a served frame whose attributes differ from the shell's out of the page", async () => {
    stubFetch();
    for (const odd of [
      `<iframe class="frame" title="artifact content" src="${START}" allow="${ALLOW}" sandbox="allow-scripts allow-same-origin"></iframe>`,
      `<iframe class="frame" title="artifact content" src="${START}" allow="clipboard-write; fullscreen"></iframe>`,
      `<iframe class="frame" title="artifact content" src="${SITE}/_clax/v/2/" allow="${ALLOW}"></iframe>`,
    ]) {
      const root = served(odd);
      const el = root.querySelector("iframe")!;
      await mountServed(root, boot());
      expect(el.isConnected, odd).toBe(false);
      const now = root.querySelector("iframe")!;
      expect(now.getAttribute("src")).toBe(START);
      expect(now.hasAttribute("sandbox")).toBe(false);
      expect(now.getAttribute("allow")).toBe(ALLOW);
      for (const v of mounted.splice(0)) v.unmount();
      root.remove();
    }
  });

  it("does not use a served frame without its bootstrap", async () => {
    stubFetch();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    await mountServed(root, null);
    expect(frame.isConnected).toBe(false);
    await waitFor(() => root.querySelector("iframe"), "the shell's own frame");
    expect(root.querySelector("iframe")).not.toBe(frame);
    expect(root.querySelector("iframe")!.getAttribute("src")).toBe(START);
  });

  it("keeps nothing early once the frame floods it, so no load in between is lost", async () => {
    new Function(EARLY)();
    const frame = served(SERVED).querySelector("iframe")!;
    for (let i = 0; i < 300; i++) fromFrame(frame.contentWindow!, i);
    const { takeEarly } = await import("./view/boot");
    expect(takeEarly()).toEqual([]);
  });

  it("keeps only the frame's messages early, so another window's flood cannot push out its hello", async () => {
    stubFetch();
    new Function(EARLY)();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    const other = document.createElement("iframe");
    document.body.append(other);
    for (let i = 0; i < 300; i++) fromFrame(other.contentWindow!, HELLO);
    fromFrame(frame.contentWindow!, HELLO);
    await mountServed(root, boot());
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
  });

  it("does not replay a frame message that a script made rather than the browser delivered", async () => {
    stubFetch();
    new Function(EARLY)();
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    window.dispatchEvent(new MessageEvent("message", { data: HELLO, origin: SITE, source: frame.contentWindow }));
    await mountServed(root, boot());
    await sleep(30);
    expect(answered(posted)).toEqual([]);
  });

  it("hands the URL's fragment to the served frame's page ahead of its welcome, and keeps it in the address bar", async () => {
    stubFetch();
    history.replaceState(null, "", `/a/${ID}#part-2`);
    const root = served(SERVED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    await mountServed(root, boot());
    expect(root.querySelector("iframe")).toBe(frame);
    // The frame is not navigated: a fragment navigation the shell starts
    // fires a load event at it in Chromium, which would close the gate.
    expect(frame.getAttribute("src")).toBe(START);
    fromFrame(frame.contentWindow!, HELLO);
    expect(posted.slice(0, 2)).toEqual([{ type: "clax:hash", hash: "#part-2" }, expect.objectContaining({ type: "clax:welcome" })]);
    expect(location.hash).toBe("#part-2");
  });
});

describe("readBoot", () => {
  afterEach(() => { document.head.replaceChildren(); });
  const block = (text: string) => {
    const s = document.createElement("script");
    s.type = "application/json";
    s.id = "clax-boot";
    s.textContent = text;
    document.head.append(s);
  };

  it("reads the daemon's block, escapes and all", async () => {
    const { readBoot } = await import("./view/boot");
    expect(readBoot()).toBeNull();
    block(JSON.stringify(boot()).replace(/</g, "\\u003c"));
    expect(readBoot()).toEqual(boot());
  });

  it("leaves out the viewer and its attention of a page reached through history, which may come from the cache", async () => {
    const { readBoot } = await import("./view/boot");
    const attention = { addressed: [], addressed_v: null, new_replies: [], open_in: ["t1"], seen: 1, looked: {} };
    block(JSON.stringify({ ...boot(), attention }));
    expect(readBoot(document, "navigate")!.viewer).toEqual(viewer);
    expect(readBoot(document, "navigate")!.attention).toEqual(attention);
    expect(readBoot(document, "back_forward")).toEqual({ ...boot(), viewer: null, attention: null });
  });

  it("refuses a block that does not parse or is not version 1", async () => {
    const { readBoot } = await import("./view/boot");
    block("{");
    expect(readBoot()).toBeNull();
    document.head.replaceChildren();
    block(JSON.stringify({ ...boot(), v: 2 }));
    expect(readBoot()).toBeNull();
  });
});
