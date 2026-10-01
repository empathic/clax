import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FRAME_SANDBOX } from "./view/frame-host";
import { SKELETON_HTML } from "./view/skeleton";

const ID = "7q3k9mzx2b4t";
const loaded = { artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "x", current_version: 2, pinned: false }, versions: [{ artifact_id: ID, n: 2, label: null, created_at: "x", files: {} }] };
const viewer = { public_id: "u_0123456789abcdef012345", display_name: null, created_at: "x" };
const boot = (frame: { mode: "subdomain" | "sandbox"; src: string } | null) => ({ v: 1 as const, artifact: loaded, threads: [], viewer, frame });
const EARLY = /<script id="clax-early">([\s\S]*?)<\/script>/.exec(readFileSync(join(__dirname, "../artifact.html"), "utf8"))![1];
const SUB = `http://${ID}.localhost:3000/v/2/`;
const SANDBOXED = `<iframe class="frame" title="artifact content" src="/c/${ID}/v/2/" allow="clipboard-write; fullscreen" sandbox="${FRAME_SANDBOX}"></iframe>`;
const SUBDOMAIN = `<iframe class="frame" title="artifact content" src="${SUB}" allow="clipboard-write; fullscreen"></iframe>`;
const HELLO = { type: "clax:hello", artifact: ID, version: 2, file: "index.html" };

class FakeES { addEventListener() {} close() {} }

async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + 2000;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}

const fromFrame = (win: Window, data: unknown, origin = "null") => window.dispatchEvent(new MessageEvent("message", { data, origin, source: win }));
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

function stubFetch(probe: () => Promise<Response>) {
  const f = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.endsWith("/healthz")) return probe();
    if (url === "/api/token") return new Response(JSON.stringify({ token: "tk" }));
    if (url.includes("/threads")) return new Response(JSON.stringify({ threads: [], next_cursor: null }));
    if (url === "/api/viewers/me") return new Response(JSON.stringify({ viewer }));
    return new Response(JSON.stringify(loaded));
  });
  vi.stubGlobal("fetch", f);
  vi.stubGlobal("EventSource", FakeES);
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

describe("the first load from the daemon's HTML", () => {
  beforeEach(async () => { vi.resetModules(); (await import("./threads")).forgetViewer(); history.replaceState(null, "", `/a/${ID}`); });
  afterEach(() => {
    for (const v of mounted.splice(0)) v.unmount();
    for (const g of gestureModules) g.unwatchShell();
    gestureModules.clear();
    (window as { __claxEarly?: { take(): unknown } }).__claxEarly?.take();
    vi.unstubAllGlobals();
    sessionStorage.clear();
    document.cookie = "clax_frame=; Max-Age=0; Path=/";
    delete (window as { __claxEarly?: unknown }).__claxEarly;
    document.body.replaceChildren();
    history.replaceState(null, "", "/");
  });

  it("replays a hello and a capability request that arrived before the shell mounted, and never fetches the artifact", async () => {
    const fetched = stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    new Function(EARLY)();
    const root = served(SANDBOXED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    fromFrame(frame.contentWindow!, HELLO);
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "early", name: "permissions" });
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    expect(root.querySelector("iframe")).toBe(frame);
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
    await waitFor(() => posted.some(m => m.type === "clax:use-result" && m.id === "early"), "the early request's answer");
    expect(posted.findIndex(m => m.type === "clax:welcome")).toBeLessThan(posted.findIndex(m => m.type === "clax:use-result"));
    const urls = fetched.mock.calls.map(c => String(c[0]));
    expect(urls).not.toContain(`/api/artifacts/${ID}`);
    expect(urls.some(u => u.includes("/threads"))).toBe(false);
    expect(urls).not.toContain("/api/viewers/me");
    expect(document.cookie).toContain("clax_frame=sandbox");
    expect(root.querySelector(".topbar h1")!.textContent).toBe("T");
  });

  it("adopting the served frame opens nothing: a request before its hello is not answered", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    new Function(EARLY)();
    const root = served(SANDBOXED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "too-soon", name: "permissions" });
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "still-too-soon", name: "permissions" });
    await sleep(30);
    expect(answered(posted)).toEqual([]);
    fromFrame(frame.contentWindow!, HELLO);
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
    expect(posted.some(m => m.type === "clax:use-result")).toBe(false);
  });

  it("counts the served frame's early load, so a later document that never greets closes the gate", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    new Function(EARLY)();
    const root = served(SANDBOXED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    fromFrame(frame.contentWindow!, HELLO);
    frame.dispatchEvent(new Event("load"));
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
    // The frame navigates to a document without the bridge: its load, with no
    // hello since the previous one, closes the gate.
    frame.dispatchEvent(new Event("load"));
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "after", name: "permissions" });
    await sleep(30);
    expect(posted.some(m => m.type === "clax:use-result")).toBe(false);
  });

  it("ignores what other windows posted before the shell mounted", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    new Function(EARLY)();
    const root = served(SANDBOXED);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    const other = document.createElement("iframe");
    document.body.append(other);
    fromFrame(other.contentWindow!, HELLO);
    fromFrame(window, HELLO);
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    await sleep(30);
    expect(answered(posted)).toEqual([]);
  });

  it("replaces a server frame whose mode the shell does not confirm, and ignores the stale frame", async () => {
    stubFetch(async () => new Response("{}"));
    new Function(EARLY)();
    const root = served(SUBDOMAIN);
    const stale = root.querySelector("iframe")!;
    const posted = posts(stale);
    fromFrame(stale.contentWindow!, HELLO, `http://${ID}.localhost:3000`);
    // This tab decided on sandbox after the page was parsed.
    sessionStorage.setItem("clax.origin-ok", "0");
    await mountServed(root, boot({ mode: "subdomain", src: SUB }));
    const now = root.querySelector("iframe")!;
    expect(now).not.toBe(stale);
    expect(stale.isConnected).toBe(false);
    expect(root.querySelectorAll("iframe")).toHaveLength(1);
    expect(now.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    expect(now.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
    expect(now.getAttribute("allow")).toBe("clipboard-write; fullscreen");
    await sleep(30);
    expect(posted).toEqual([]);
    expect(document.cookie).toContain("clax_frame=sandbox");
  });

  it("removes a served subdomain frame while the page is parsed, in a tab whose probe said no", async () => {
    sessionStorage.setItem("clax.origin-ok", "0");
    new Function(EARLY)();
    const root = served(SUBDOMAIN);
    const sandboxed = served(SANDBOXED);
    await Promise.resolve();
    expect(root.querySelector("iframe")).toBeNull();
    expect(sandboxed.querySelector("iframe")).not.toBeNull();
  });

  it("keeps a served frame whose attributes differ from the shell's out of the page", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    const root = served(`<iframe class="frame" title="artifact content" src="/c/${ID}/v/2/" allow="clipboard-write; fullscreen" sandbox="allow-scripts allow-same-origin"></iframe>`);
    const odd = root.querySelector("iframe")!;
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    expect(odd.isConnected).toBe(false);
    expect(root.querySelector("iframe")!.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
  });

  it("does not use a served frame without its bootstrap", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    const root = served(SANDBOXED);
    const frame = root.querySelector("iframe")!;
    await mountServed(root, null);
    expect(frame.isConnected).toBe(false);
    await waitFor(() => root.querySelector("iframe"), "the shell's own frame");
    expect(root.querySelector("iframe")).not.toBe(frame);
  });

  it("keeps the served frame when the probe confirms the guess, and replaces it when the probe fails", async () => {
    stubFetch(async () => new Response("{}"));
    const kept = served(SUBDOMAIN);
    const first = kept.querySelector("iframe")!;
    const view = await mountServed(kept, boot({ mode: "subdomain", src: SUB }));
    expect(kept.querySelector("iframe")).toBe(first);
    await waitFor(() => sessionStorage.getItem("clax.origin-ok") === "1", "the probe");
    expect(kept.querySelector("iframe")).toBe(first);
    expect(document.cookie).toContain("clax_frame=subdomain");
    view.unmount();
    sessionStorage.clear();
    vi.resetModules();
    stubFetch(async () => { throw new TypeError("no such host"); });
    const replaced = served(SUBDOMAIN);
    const guess = replaced.querySelector("iframe")!;
    await mountServed(replaced, boot({ mode: "subdomain", src: SUB }));
    await waitFor(() => replaced.querySelector("iframe") !== guess, "the replacement");
    expect(replaced.querySelector("iframe")!.getAttribute("src")).toBe(`/c/${ID}/v/2/`);
    expect(replaced.querySelector("iframe")!.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
    expect(document.cookie).toContain("clax_frame=sandbox");
  });

  it("hears a frame adopted on a subdomain guess only once this tab's probe agrees", async () => {
    let answer!: (r: Response) => void;
    stubFetch(() => new Promise<Response>(r => { answer = r; }));
    new Function(EARLY)();
    const root = served(SUBDOMAIN);
    const frame = root.querySelector("iframe")!;
    const posted = posts(frame);
    const origin = `http://${ID}.localhost:3000`;
    fromFrame(frame.contentWindow!, HELLO, origin);
    await mountServed(root, boot({ mode: "subdomain", src: SUB }));
    expect(root.querySelector("iframe")).toBe(frame);
    fromFrame(frame.contentWindow!, { type: "clax:use", id: "u1", name: "permissions" }, origin);
    await sleep(30);
    expect(answered(posted)).toEqual([]);
    await waitFor(() => answer, "the probe");
    answer(new Response("{}"));
    await waitFor(() => posted.some(m => m.type === "clax:use-result" && m.id === "u1"), "the held request's answer");
    expect(posted.findIndex(m => m.type === "clax:welcome")).toBeLessThan(posted.findIndex(m => m.type === "clax:use-result"));
  });

  it("never answers a frame adopted on a subdomain guess that the probe then refuses", async () => {
    let fail!: (e: Error) => void;
    stubFetch(() => new Promise<Response>((_, r) => { fail = r; }));
    new Function(EARLY)();
    const root = served(SUBDOMAIN);
    const guess = root.querySelector("iframe")!;
    const posted = posts(guess);
    const origin = `http://${ID}.localhost:3000`;
    fromFrame(guess.contentWindow!, HELLO, origin);
    await mountServed(root, boot({ mode: "subdomain", src: SUB }));
    fromFrame(guess.contentWindow!, { type: "clax:use", id: "u1", name: "permissions" }, origin);
    await waitFor(() => fail, "the probe");
    fail(new TypeError("blocked"));
    await waitFor(() => root.querySelector("iframe") !== guess, "the replacement");
    await sleep(30);
    expect(answered(posted)).toEqual([]);
    expect(root.querySelector("iframe")!.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
  });

  it("keeps nothing early once a page floods it, so no load in between is lost", async () => {
    new Function(EARLY)();
    for (let i = 0; i < 300; i++) window.dispatchEvent(new MessageEvent("message", { data: i }));
    const { takeEarly } = await import("./view/boot");
    expect(takeEarly()).toEqual([]);
  });

  it("opens the served frame at the URL's fragment and keeps it in the address bar", async () => {
    stubFetch(async () => new Response("{}"));
    sessionStorage.setItem("clax.origin-ok", "0");
    history.replaceState(null, "", `/a/${ID}#part-2`);
    const root = served(SANDBOXED);
    const frame = root.querySelector("iframe")!;
    await mountServed(root, boot({ mode: "sandbox", src: `/c/${ID}/v/2/` }));
    expect(root.querySelector("iframe")).toBe(frame);
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/2/#part-2`);
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
    block(JSON.stringify(boot(null)).replace(/</g, "\\u003c"));
    expect(readBoot()).toEqual(boot(null));
  });

  it("leaves out the viewer of a page reached through history, which may come from the cache", async () => {
    const { readBoot } = await import("./view/boot");
    block(JSON.stringify(boot(null)));
    expect(readBoot(document, "navigate")!.viewer).toEqual(viewer);
    expect(readBoot(document, "back_forward")).toEqual({ ...boot(null), viewer: null });
  });

  it("refuses a block that does not parse or is not version 1", async () => {
    const { readBoot } = await import("./view/boot");
    block("{");
    expect(readBoot()).toBeNull();
    document.head.replaceChildren();
    block(JSON.stringify({ ...boot(null), v: 2 }));
    expect(readBoot()).toBeNull();
  });
});
