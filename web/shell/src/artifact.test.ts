import { describe, it, expect, vi, beforeEach, afterEach, onTestFinished } from "vitest";
import { dispatchTrusted } from "../../bridge/test/trusted";

class FakeES {
  static last: FakeES | undefined;
  listeners = new Map<string, (e: MessageEvent) => void>();
  constructor(public url: string) { FakeES.last = this; }
  addEventListener(t: string, fn: (e: MessageEvent) => void) { this.listeners.set(t, fn); }
  close() {}
  emit(t: string, data: unknown) { this.listeners.get(t)?.(new MessageEvent(t, { data: JSON.stringify(data) })); }
}

async function waitFor<T>(check: () => T | null | undefined | false, what: string): Promise<T> {
  const deadline = Date.now() + 2000;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}

const ID = "7q3k9mzx2b4t";
const artifact = (n: number, files: Record<string, unknown> = {}) => ({ artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "2026-09-28T11:00:00Z", current_version: n, pinned: false, owner_harness: "claude" }, versions: [{ artifact_id: ID, n, label: null, created_at: "x", files }] });
const page = { content_type: "text/html", size: 1 };

const viewer = { viewer: { public_id: "u_0123456789abcdef012345", display_name: null, created_at: "x" } };

/** The gesture module the mounted view uses (from the same module registry). */
let gesture: typeof import("./caps/gesture") | undefined;
/** Every gesture module instance a mount loaded: each watches the document
 * until its `unwatchShell` runs. */
const gestureModules = new Set<typeof import("./caps/gesture")>();
afterEach(() => { for (const g of gestureModules) g.unwatchShell(); gestureModules.clear(); });
/** Every view a test mounted and left mounted; each is unmounted after its test, so its timers stop. */
const mountedViews = new Set<() => void>();
afterEach(() => { for (const stop of mountedViews) stop(); mountedViews.clear(); });

/** Answers the comment routes (no threads, an anonymous viewer) unless `comments` is given; everything else goes to `fetchImpl`. */
async function mountView(fetchImpl: (url: string, init?: RequestInit) => Promise<Response>, comments?: (url: string, init?: RequestInit) => Promise<Response>, file?: string, pinned: number | null = null) {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes("/threads") || url.startsWith("/api/viewers/")) {
      if (comments) return comments(url, init);
      return new Response(JSON.stringify(url.includes("/threads") ? { threads: [], next_cursor: null } : viewer));
    }
    return fetchImpl(url, init);
  }));
  sessionStorage.setItem("clax.origin-ok", "0");
  const { mountArtifactView } = await import("./artifact");
  gesture = await import("./caps/gesture");
  gestureModules.add(gesture);
  const root = document.createElement("div");
  document.body.appendChild(root);
  const view = mountArtifactView(root, { id: ID, pinnedVersion: pinned, file });
  let stopped = false;
  const stop = () => { if (!stopped) { stopped = true; mountedViews.delete(stop); view.unmount(); } };
  mountedViews.add(stop);
  return { ...view, root: view.root, unmount: stop };
}

describe("ArtifactView", () => {
  // `forgetViewer` comes from the fresh module registry, the same `threads`
  // module the test's `import("./artifact")` then loads.
  beforeEach(async () => { vi.resetModules(); (await import("./threads")).forgetViewer(); FakeES.last = undefined; });
  beforeEach(() => { history.replaceState(null, "", `/a/${ID}`); });
  afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); history.replaceState(null, "", "/"); });

  it("disposes the capability host when it is replaced and on unmount", async () => {
    const view = await mountView(async () => new Response(JSON.stringify({ artifact: artifact(2).artifact, versions: [...artifact(1).versions, ...artifact(2).versions] })));
    const root = view.root;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    const { CapabilityHost } = await import("./caps/host");
    const dispose = vi.spyOn(CapabilityHost.prototype, "dispose");
    view.update({ id: ID, pinnedVersion: 1, file: undefined });
    await waitFor(() => dispose.mock.calls.length === 1, "the replaced host's dispose");
    view.unmount();
    await waitFor(() => dispose.mock.calls.length === 2, "dispose on unmount");
    expect(new Set(dispose.mock.instances).size).toBe(2);
  });

  it("follows a page publish at once on the page it shows, unless pinned", async () => {
    const assign = vi.fn();
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1, { "index.html": page, "about.html": page }))));
    const root = view.root;
    (await import("./nav")).nav.assign = assign;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    fromFrame(frame.contentWindow!, { type: "clax:hello", artifact: ID, version: 1, file: "about.html" });
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "the about page's URL");
    const es = await waitFor(() => FakeES.last, "event stream");
    es.emit("version", { type: "version", artifact_id: ID, n: 2, by_page: true });
    await waitFor(() => assign.mock.calls.length === 1, "the reload");
    expect(assign).toHaveBeenCalledWith(`/a/${ID}/about.html`);
    // The page caused the load, so the next view starts with the keys held.
    expect(sessionStorage.getItem("clax.keys-held")).toBe("1");
    expect(root.querySelector("button.reload")).toBeNull();
    // An agent's publish still offers Reload in the top bar.
    es.emit("version", { type: "version", artifact_id: ID, n: 3 });
    await waitFor(() => root.querySelector("button.reload"), "the Reload button");
    expect(assign).toHaveBeenCalledTimes(1);
  });

  it("a pinned view offers Reload for a page publish and does not reload", async () => {
    const assign = vi.fn();
    history.replaceState(null, "", `/a/${ID}/v/1`);
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))), undefined, undefined, 1);
    const root = view.root;
    (await import("./nav")).nav.assign = assign;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    (await waitFor(() => FakeES.last, "event stream")).emit("version", { type: "version", artifact_id: ID, n: 2, by_page: true });
    await waitFor(() => root.querySelector("button.reload"), "the Reload button");
    expect(assign).not.toHaveBeenCalled();
    expect(sessionStorage.getItem("clax.keys-held")).toBeNull();
  });

  it("holds another view's page publish until this view's own publish settles", async () => {
    const assign = vi.fn();
    let answer!: (r: Response) => void;
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { artifact: {} } } };
    const view = await mountView(async (url, init) => {
      if (url === "/api/token") return new Response(JSON.stringify({ token: "tk" }));
      if (init?.method === "POST") return new Promise<Response>(r => { answer = r; });
      return new Response(JSON.stringify(declared));
    });
    const root = view.root;
    (await import("./nav")).nav.assign = assign;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; ok?: boolean }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    gestureIn(frame);
    fromFrame(win, { type: "clax:call", id: "p1", ns: "artifact", method: "publish", args: ["<!doctype html><p>2"] });
    await waitFor(() => answer, "the publish request");
    const es = await waitFor(() => FakeES.last, "event stream");
    es.emit("version", { type: "version", artifact_id: ID, n: 2, by_page: true });
    await new Promise(r => setTimeout(r, 30));
    expect(assign).not.toHaveBeenCalled();
    answer(new Response(JSON.stringify({ error: { code: "internal", message: "down" } }), { status: 500 }));
    await waitFor(() => posted.some(m => m.type === "clax:call-result" && m.id === "p1" && m.ok === false), "the publish result");
    await waitFor(() => assign.mock.calls.length === 1, "the held reload");
    expect(assign).toHaveBeenCalledWith(`/a/${ID}`);
    expect(sessionStorage.getItem("clax.keys-held")).toBe("1");
  });

  it("reloads after the page's own publish with the keys held for the next view", async () => {
    // An earlier test's activation would count as input before the shell's script ran.
    Object.defineProperty(navigator, "userActivation", { value: { isActive: false }, configurable: true });
    const assign = vi.fn();
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { artifact: {} } } };
    const view = await mountView(async (url, init) => {
      if (url === "/api/token") return new Response(JSON.stringify({ token: "tk" }));
      if (init?.method === "POST") return new Response(JSON.stringify({ version: { n: 2 } }), { status: 201 });
      return new Response(JSON.stringify(declared));
    });
    const root = view.root;
    (await import("./nav")).nav.assign = assign;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; ok?: boolean }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    gestureIn(frame);
    fromFrame(win, { type: "clax:call", id: "p1", ns: "artifact", method: "publish", args: ["<!doctype html><p>2"] });
    await waitFor(() => posted.some(m => m.type === "clax:call-result" && m.id === "p1" && m.ok === true), "the publish result");
    expect(sessionStorage.getItem("clax.keys-held")).toBeNull();
    await waitFor(() => assign.mock.calls.length === 1, "the reload");
    expect(assign).toHaveBeenCalledWith(`/a/${ID}`);
    expect(sessionStorage.getItem("clax.keys-held")).toBe("1");
    view.unmount();
  });

  it("drops another view's held page publish when it unmounts during its own publish", async () => {
    // An earlier test's activation would count as input before the shell's script ran.
    Object.defineProperty(navigator, "userActivation", { value: { isActive: false }, configurable: true });
    const assign = vi.fn();
    let answer!: (r: Response) => void;
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { artifact: {} } } };
    const view = await mountView(async (url, init) => {
      if (url === "/api/token") return new Response(JSON.stringify({ token: "tk" }));
      if (init?.method === "POST") return new Promise<Response>(r => { answer = r; });
      return new Response(JSON.stringify(declared));
    });
    const root = view.root;
    (await import("./nav")).nav.assign = assign;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    gestureIn(frame);
    fromFrame(win, { type: "clax:call", id: "p1", ns: "artifact", method: "publish", args: ["<!doctype html><p>2"] });
    await waitFor(() => answer, "the publish request");
    (await waitFor(() => FakeES.last, "event stream")).emit("version", { type: "version", artifact_id: ID, n: 2, by_page: true });
    // Leaving the artifact ends its own publish; the held reload belonged to it.
    view.unmount();
    answer(new Response(JSON.stringify({ error: { code: "internal", message: "down" } }), { status: 500 }));
    await new Promise(r => setTimeout(r, 30));
    expect(assign).not.toHaveBeenCalled();
  });

  it("says not found only for a 404 status", async () => {
    const view = await mountView(async () => new Response(JSON.stringify({ error: { message: "nope" } }), { status: 404 }));
    const root = view.root;
    await waitFor(() => root.textContent?.includes("Artifact not found"), "not-found message");
  });

  it("shows the raw error for other failures, even when the text contains 404", async () => {
    const view = await mountView(async () => new Response(JSON.stringify({ error: { message: "port 4040 or 404 busy" } }), { status: 500 }));
    const root = view.root;
    await waitFor(() => root.textContent?.includes("port 4040"), "error message");
    expect(root.textContent).not.toContain("Artifact not found");
  });

  it("refetches on resync and offers Reload when a newer version exists", async () => {
    let current = 1;
    const view = await mountView(async () => new Response(JSON.stringify(artifact(current))));
    const root = view.root;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    expect(root.querySelector("button.reload")).toBeNull();
    (await waitFor(() => FakeES.last, "event stream")).emit("resync", { dropped: 3 });
    await new Promise(r => setTimeout(r, 30));
    expect(root.querySelector("button.reload")).toBeNull();
    current = 4;
    (await waitFor(() => FakeES.last, "event stream")).emit("resync", { dropped: 3 });
    await waitFor(() => root.querySelector("button.reload"), "the Reload button");
  });

  it("reloads the threads when the event stream (re)connects", async () => {
    let listed = 0;
    const t = { id: "01JA", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "01JA", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "made while the daemon restarted", created_at: "x" }] };
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async url => url.includes("/threads")
        ? new Response(JSON.stringify({ threads: listed++ === 0 ? [] : [t], next_cursor: null }))
        : new Response(JSON.stringify(viewer)));
    const root = view.root;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    await waitFor(() => listed === 1, "initial thread load");
    expect(buttonNamed(root, /^Threads/).textContent).toBe("Threads 0");
    (await waitFor(() => FakeES.last, "event stream")).emit("ready", {});
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads 1", "thread from the reload");
  });

  it("opens the event stream only after the viewer lookup answered, with the owner shell's token", async () => {
    let answerViewer!: () => void;
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1))),
      url => url.includes("/threads")
        ? Promise.resolve(new Response(JSON.stringify({ threads: [], next_cursor: null })))
        : new Promise<Response>(r => { answerViewer = () => r(new Response(JSON.stringify(viewer))); }));
    const root = view.root;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    await new Promise(r => setTimeout(r, 30));
    expect(FakeES.last).toBeUndefined();
    answerViewer();
    const es = await waitFor(() => FakeES.last, "event stream");
    expect(es.url).toBe(`/api/events?artifact=${ID}&token=tk`);
  });

  it("keeps a thread event that arrives while an older thread list is in flight", async () => {
    let answerList!: () => void;
    const t = { id: "01JB", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] };
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      url => url.includes("/threads")
        ? new Promise<Response>(r => { answerList = () => r(new Response(JSON.stringify({ threads: [], next_cursor: null }))); })
        : Promise.resolve(new Response(JSON.stringify(viewer))));
    const root = view.root;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    (await waitFor(() => FakeES.last, "event stream")).emit("thread", { type: "thread", artifact_id: ID, thread: t });
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads 1", "thread from the event");
    answerList();
    await new Promise(r => setTimeout(r, 30));
    expect(buttonNamed(root, /^Threads/).textContent).toBe("Threads 1");
  });

  it("shows a failed thread load in the notice banner", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async url => url.includes("/threads")
        ? new Response(JSON.stringify({ error: { code: "internal", message: "db locked" } }), { status: 500 })
        : new Response(JSON.stringify(viewer)));
    const root = view.root;
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.getAttribute("role")).toBe("alert");
    expect(banner.textContent).toContain("Could not load comments: 500 db locked");
  });

  it("says a failed name lookup could not load the name", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async url => url.includes("/threads")
        ? new Response(JSON.stringify({ threads: [], next_cursor: null }))
        : new Response(JSON.stringify({ error: { code: "forbidden_origin", message: "nope" } }), { status: 403 }));
    const root = view.root;
    (await import("./ui/more-menu.svelte")).loadMoreMenu();
    await waitFor(() => !root.querySelector(".more-slot"), "more menu");
    (await waitFor(() => root.querySelector<HTMLButtonElement>("button.who"), "roster")).click();
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.textContent).toContain("Could not load your name: 403 nope");
  });

  for (const narrow of [false, true]) {
    it(`puts "Your name" in the people panel, opened from the roster (${narrow ? "narrow" : "wide"})`, async () => {
      stubMedia({ "(min-width: 900px)": !narrow, "(max-width: 480px)": narrow });
      const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
      const root = view.root;
      await waitFor(() => root.querySelector("iframe.frame"), "viewer");
      expect(root.querySelector('input[aria-label="Your name"]')).toBeNull();
      // The entry loads the roster after the first paint (`artifact-main.ts`).
      (await import("./ui/more-menu.svelte")).loadMoreMenu();
      await waitFor(() => !root.querySelector(".more-slot"), "more menu");
      const who = await waitFor(() => root.querySelector<HTMLButtonElement>("button.who"), "roster");
      expect(who.getAttribute("aria-label")).toBe("People and agents");
      who.click();
      await waitFor(() => root.querySelector('[role="dialog"][aria-label="People and agents"] input[aria-label="Your name"]'), "name field in the people panel");
      expect(root.querySelectorAll('input[aria-label="Your name"]')).toHaveLength(1);
      expect(who.getAttribute("aria-expanded")).toBe("true");
      // The thread list, which loads after the first paint, lands in this test's module registry too.
      if (!narrow) await waitFor(() => root.querySelector('aside.sidebar:not([aria-busy])'), "thread list");
    });
  }

  it("ignores a hello from another artifact or version, and welcomes the shown one", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(2))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: unknown[] = [];
    win.postMessage = ((m: unknown) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    fromFrame(win, { type: "clax:hello", artifact: "9zzzzzzzzzzz", version: 2, file: "index.html" });
    await new Promise(r => setTimeout(r, 20));
    expect(posted.filter(m => (m as { type: string }).type === "clax:welcome")).toHaveLength(0);
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" });
    await waitFor(() => posted.some(m => (m as { type: string }).type === "clax:welcome"), "welcome");
  });

  it("welcomes a hello, and answers a capability request, that arrive as soon as the frame is inserted", async () => {
    const posted: { type: string; id?: string }[] = [];
    const seen = new MutationObserver(() => {
      const frame = document.querySelector<HTMLIFrameElement>("iframe.frame");
      if (!frame) return;
      seen.disconnect();
      const win = frame.contentWindow!;
      win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
      // Before the render that inserted the frame has run its effects.
      fromFrame(win, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" });
      fromFrame(win, { type: "clax:use", id: "early", name: "permissions" });
    });
    seen.observe(document.body, { subtree: true, childList: true });
    await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(2))));
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    await waitFor(() => posted.some(m => m.type === "clax:use-result" && m.id === "early"), "the answer to the early request");
  });

  it("answers capability requests only after a hello for the shown artifact and version", async () => {
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(2))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    const results = () => posted.filter(m => m.type === "clax:use-result").map(m => m.id);
    await new Promise(r => setTimeout(r, 30));
    fromFrame(win, { type: "clax:use", id: "before", name: "permissions" });
    await new Promise(r => setTimeout(r, 30));
    expect(results()).toEqual([]);
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" });
    fromFrame(win, { type: "clax:use", id: "matched", name: "permissions" });
    await waitFor(() => results().includes("matched"), "use answer after the hello");
    // The frame navigated to another document, which greets as something else.
    fromFrame(win, { type: "clax:hello", artifact: "9zzzzzzzzzzz", version: 2, file: "index.html" });
    fromFrame(win, { type: "clax:use", id: "foreign", name: "permissions" });
    fromFrame(win, { type: "clax:call", id: "foreign-call", ns: "permissions", method: "state", args: [] });
    await new Promise(r => setTimeout(r, 30));
    expect(results()).toEqual(["matched"]);
    expect(posted.some(m => m.type === "clax:call-result")).toBe(false);
  });

  it("follows the page the frame shows: resolves its threads only, and opens another page's thread on that page", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = (id: string, file: string) => ({ id, artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: `Goals ${id}`, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body: `note ${id}`, created_at: "x" }] });
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "about.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t("tI", "index.html"), t("tA", "about.html")], next_cursor: null } : viewer)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const posted: { type: string; anchors?: { id: string; anchor: { quote: string } }[]; anchor?: { file: string } }[] = [];
    // jsdom gives the frame a new window when it navigates; a browser keeps one WindowProxy.
    const tap = () => { const w = frame.contentWindow!; w.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof w.postMessage; return w; };
    let win = tap();
    // Anchors go under opaque handles, never the threads' store IDs; the quote names the thread here.
    const lastResolve = () => {
      const anchors = posted.filter(m => m.type === "clax:resolve-anchors").at(-1)?.anchors;
      for (const a of anchors ?? []) expect(a.id).toMatch(/^a[0-9a-f]{24}$/);
      return anchors?.map(a => a.anchor.quote.replace("Goals ", ""));
    };
    await waitFor(() => root.querySelector('[data-thread="tA"]'), "threads in the sidebar");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => lastResolve()?.join() === "tI", "resolution of the index's threads only");
    const card = root.querySelector('[data-thread="tA"]')!;
    expect(card.querySelector(".file-label")!.textContent).toBe("on about.html");
    card.querySelector<HTMLButtonElement>("button.card-head")!.click();
    // One history entry: the shell URL is pushed and the frame is moved in place.
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "the shell URL names about.html");
    expect(posted.some(m => m.type === "clax:scroll-to")).toBe(false);
    win = tap();
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "about.html" });

    await waitFor(() => lastResolve()?.join() === "tA", "resolution of about.html's threads");
    const scroll = await waitFor(() => posted.find(m => m.type === "clax:scroll-to"), "scroll to the thread once its page greeted");
    expect(scroll.anchor!.file).toBe("about.html");
    await waitFor(() => !root.querySelector('[data-thread="tA"] .file-label') && root.querySelector('[data-thread="tI"] .file-label'), "labels follow the page");
  });

  it("opens the frame on the URL's page, and says so when the version does not hold it", async () => {
    let view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "docs/about.html": page }))), undefined, "docs/about.html");
    let root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/1/docs/about.html`);
    view.unmount();
    document.body.replaceChildren();
    vi.resetModules();
    view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))), undefined, "gone.html");
    root = view.root;
    const msg = await waitFor(() => root.querySelector(".stage .empty"), "not-found message");
    expect(msg.textContent).toContain("v1 has no page gone.html");
    expect(msg.querySelector("a")!.getAttribute("href")).toBe(`/a/${ID}`);
    expect(root.querySelector("iframe")).toBeNull();
  });

  it("opens the frame at the URL's fragment, and keeps the address bar's fragment in step with the frame's", async () => {
    history.replaceState(null, "", `/a/${ID}/about.html#docs%2Fcontract.md`);
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "about.html": page }))), undefined, "about.html");
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/1/about.html#docs%2Fcontract.md`);
    const win = frame.contentWindow!;
    win.postMessage = (() => {}) as typeof win.postMessage;
    const depth = history.length;
    const settle = () => new Promise(r => setTimeout(r, 20));
    fromFrame(win, { type: "clax:hash", hash: "#early" });
    await settle();
    expect(location.hash).toBe("#docs%2Fcontract.md");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "about.html" });
    fromFrame(win, { type: "clax:hash", hash: "#crates%2Fa.rs" });
    await waitFor(() => location.hash === "#crates%2Fa.rs", "the frame's fragment in the address bar");
    expect([location.pathname, history.length]).toEqual([`/a/${ID}/about.html`, depth]);
    for (const bad of ["no-hash", 42, `#${"x".repeat(512)}`]) fromFrame(win, { type: "clax:hash", hash: bad });
    await settle();
    expect(location.hash).toBe("#crates%2Fa.rs");
    fromFrame(win, { type: "clax:hash", hash: "" });
    await waitFor(() => location.hash === "", "no fragment");
    expect(location.pathname).toBe(`/a/${ID}/about.html`);
  });

  it("copies a burst of frame fragments into the address bar once per animation frame, the latest one", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    win.postMessage = (() => {}) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await new Promise(r => setTimeout(r, 20));
    const replace = vi.spyOn(history, "replaceState");
    try {
      for (let i = 0; i < 40; i++) fromFrame(win, { type: "clax:hash", hash: `#h${i}` });
      await waitFor(() => location.hash === "#h39", "the latest fragment");
      expect(replace).toHaveBeenCalledTimes(1);
    } finally {
      replace.mockRestore();
    }
  });

  it("survives a browser that refuses history calls, and still moves the frame to a handed-over page", async () => {
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1, { "index.html": page, "about.html": page }))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    const refuse = () => { throw new DOMException("Attempt to use history.pushState() more than 100 times per 10 seconds", "SecurityError"); };
    const push = vi.spyOn(history, "pushState").mockImplementation(refuse);
    const replace = vi.spyOn(history, "replaceState").mockImplementation(refuse);
    const errors: unknown[] = [];
    const onError = (e: ErrorEvent) => { errors.push(e.error); };
    window.addEventListener("error", onError);
    try {
      fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
      await waitFor(() => posted.some(m => m.type === "clax:welcome"), "the welcome");
      fromFrame(win, { type: "clax:hash", hash: "#x" });
      await new Promise(r => setTimeout(r, 40));
      fromFrame(win, { type: "clax:navigate", file: "about.html" });
      await waitFor(() => frame.getAttribute("src") === `/c/${ID}/v/1/about.html`, "the frame on the about page");
      expect(push).toHaveBeenCalled();
      expect(replace).toHaveBeenCalled();
      expect(errors).toEqual([]);
      expect(location.pathname).toBe(`/a/${ID}`);
    } finally {
      window.removeEventListener("error", onError);
      push.mockRestore();
      replace.mockRestore();
    }
  });

  it("puts the page the frame greets from in the address bar, and ignores a page the version does not hold", async () => {
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1, { "index.html": page, "about.html": page }))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await new Promise(r => setTimeout(r, 20));
    expect(location.pathname).toBe(`/a/${ID}`);
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "about.html" });
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "the about page's URL");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => location.pathname === `/a/${ID}`, "back on the index's URL");
    // A page the shown version does not hold: no address change, and no answers.
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "forged.html" });
    fromFrame(win, { type: "clax:use", id: "forged", name: "permissions" });
    await new Promise(r => setTimeout(r, 30));
    expect(location.pathname).toBe(`/a/${ID}`);
    expect(posted.some(m => m.type === "clax:use-result" && m.id === "forged")).toBe(false);
  });

  it("drops the pins when the frame loads a document that never greets", async () => {
    const t = { id: "tI", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] };
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const sent: { type: string; anchors?: { id: string }[] }[] = [];
    win.postMessage = ((m: (typeof sent)[number]) => { sent.push(m); }) as typeof win.postMessage;
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads 1", "thread loaded");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const handle = (await waitFor(() => sent.filter(m => m.type === "clax:resolve-anchors").at(-1)?.anchors?.[0], "anchors sent")).id;
    expect(handle).not.toBe("tI");
    // A result naming the store ID (which the frame never learns) places nothing.
    fromFrame(win, { type: "clax:anchors", requestId: "r0", results: [{ id: "tI", found: true, method: "exact", rect: { x: 10, y: 40, w: 100, h: 20 } }] });
    await new Promise(r => setTimeout(r, 20));
    expect(root.querySelector("button.thread-pin")).toBeNull();
    fromFrame(win, { type: "clax:anchors", requestId: "r1", results: [{ id: handle, found: true, method: "exact", rect: { x: 10, y: 40, w: 100, h: 20 } }] });
    await waitFor(() => root.querySelector("button.thread-pin"), "the pin");
    // Every greeting page gets new handles; the old ones no longer place pins.
    const count = sent.filter(m => m.type === "clax:resolve-anchors").length;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const again = (await waitFor(() => sent.filter(m => m.type === "clax:resolve-anchors")[count]?.anchors?.[0], "anchors sent again")).id;
    expect(again).not.toBe(handle);
    fromFrame(win, { type: "clax:anchors", requestId: "r2", results: [{ id: handle, found: true, method: "exact", rect: { x: 10, y: 40, w: 100, h: 20 } }] });
    await waitFor(() => !root.querySelector("button.thread-pin"), "a stale handle places nothing");
    fromFrame(win, { type: "clax:anchors", requestId: "r3", results: [{ id: again, found: true, method: "exact", rect: { x: 10, y: 40, w: 100, h: 20 } }] });
    await waitFor(() => root.querySelector("button.thread-pin"), "the pin again");
    frame.dispatchEvent(new Event("load"));
    expect(root.querySelector("button.thread-pin")).not.toBeNull();
    frame.dispatchEvent(new Event("load"));
    await waitFor(() => !root.querySelector("button.thread-pin"), "no pin over a document without the bridge");
  });

  it("lists a thread on a page the version does not hold as Detached, and opening it stays put", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tG", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Gone", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "gone.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] };
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const card = await waitFor(() => root.querySelector('.section-detached [data-thread="tG"]'), "the detached card");
    card.querySelector<HTMLButtonElement>("button.card-head")!.click();
    await new Promise(r => setTimeout(r, 30));
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/1/`);
    expect(location.pathname).toBe(`/a/${ID}`);
  });

  it("follows a link the page handed over as one history entry per greeting page, with its fragment", async () => {
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1, { "index.html": page, "about.html": page, "doc.pdf": { content_type: "application/pdf", size: 1 } }))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const posted: { type: string; id?: string }[] = [];
    // jsdom gives the frame a new window when its src changes; a browser keeps one WindowProxy.
    const tap = () => { const w = frame.contentWindow!; w.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof w.postMessage; return w; };
    let win = tap();
    const settle = () => new Promise(r => setTimeout(r, 20));
    fromFrame(win, { type: "clax:navigate", file: "about.html" });
    await settle();
    expect(location.pathname).toBe(`/a/${ID}`);
    const depth = history.length;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    fromFrame(win, { type: "clax:navigate", file: "about.html", hash: "#team" });
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "the about page's URL");
    expect(location.hash).toBe("#team");
    // The outgoing document is no longer answered: a burst of links, a capability request.
    fromFrame(win, { type: "clax:navigate", file: "index.html" });
    fromFrame(win, { type: "clax:use", id: "stale", name: "permissions" });
    await settle();
    expect([location.pathname, history.length]).toEqual([`/a/${ID}/about.html`, depth + 1]);
    expect(posted.some(m => m.id === "stale")).toBe(false);
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "about.html" });
    await settle();
    expect(location.hash).toBe("#team");
    // Not an HTML page: the frame loads it in place, the URL stays.
    fromFrame(win, { type: "clax:navigate", file: "doc.pdf" });
    await waitFor(() => frame.getAttribute("src") === `/c/${ID}/v/1/doc.pdf`, "the PDF in the frame");
    expect(location.pathname).toBe(`/a/${ID}/about.html`);
    win = tap();
    // Malformed requests are ignored: a non-string, a path outside the version, a missing page.
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "about.html" });
    for (const bad of [{ file: 42 }, { file: "../index.html" }, { file: "missing.html" }]) fromFrame(win, { type: "clax:navigate", ...bad });
    await settle();
    expect([location.pathname, history.length]).toEqual([`/a/${ID}/about.html`, depth + 1]);
    // A fragment that is not one, or is too long, is dropped.
    fromFrame(win, { type: "clax:navigate", file: "index.html", hash: "team" });
    await waitFor(() => location.pathname === `/a/${ID}`, "the index");
    expect(location.hash).toBe("");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    fromFrame(win, { type: "clax:navigate", file: "about.html", hash: `#${"x".repeat(512)}` });
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "about again");
    expect(location.hash).toBe("");
  });

  it("says so when the page of an opened thread never greets, and not when the viewer moved on", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tA", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Team", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "about.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] };
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "about.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
    const root = view.root;
    const wait = (await import("./artifact")).pageWait;
    const before = wait.ms;
    wait.ms = 50;
    try {
      const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
      const win = frame.contentWindow!;
      win.postMessage = (() => {}) as typeof win.postMessage;
      const open = async () => {
        const card = await waitFor(() => root.querySelector('[data-thread="tA"]'), "the card");
        card.querySelector<HTMLButtonElement>("button.card-head")!.click();
        await waitFor(() => location.pathname === `/a/${ID}/about.html`, "navigated");
      };
      // A fast Back abandons the jump without a notice.
      await open();
      history.back();
      await waitFor(() => location.pathname === `/a/${ID}`, "back on the index URL");
      await new Promise(r => setTimeout(r, 120));
      expect(root.querySelector(".banner.notice")).toBeNull();
      // Another page greeting instead abandons it too.
      await open();
      fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
      await new Promise(r => setTimeout(r, 120));
      expect(root.querySelector(".banner.notice")).toBeNull();
      // Nothing greets: the notice.
      await open();
      const banner = await waitFor(() => root.querySelector(".banner.notice"), "failure banner");
      expect(banner.textContent).toContain("Could not open about.html");
    } finally {
      wait.ms = before;
    }
  });

  it("closes the capability gate on a frame load that no hello preceded", async () => {
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(2))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    const results = () => posted.filter(m => m.type === "clax:use-result").map(m => m.id);
    await new Promise(r => setTimeout(r, 30));
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" });
    frame.dispatchEvent(new Event("load"));
    fromFrame(win, { type: "clax:use", id: "greeted", name: "permissions" });
    await waitFor(() => results().includes("greeted"), "a load right after the hello keeps the gate open");
    // The frame navigated to a document without a bridge: it loads without greeting.
    frame.dispatchEvent(new Event("load"));
    fromFrame(win, { type: "clax:use", id: "silent", name: "permissions" });
    await new Promise(r => setTimeout(r, 30));
    expect(results()).toEqual(["greeted"]);
  });

  it("drops the thread changes it kept once the latest list answered or failed", async () => {
    let lists = 0;
    let failNext = false;
    const t = (id: string) => ({ id, artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] });
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async url => {
        if (!url.includes("/threads")) return new Response(JSON.stringify(viewer));
        lists++;
        if (failNext) return new Response(JSON.stringify({ error: { code: "internal", message: "db locked" } }), { status: 500 });
        return new Response(JSON.stringify({ threads: [], next_cursor: null }));
      });
    const root = view.root;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    const es = await waitFor(() => FakeES.last, "event stream");
    await waitFor(() => lists >= 1, "initial list");
    await new Promise(r => setTimeout(r, 20));
    // Answered: an event now is not replayed onto the next list, which no longer has it.
    es.emit("thread", { type: "thread", artifact_id: ID, thread: t("01JA") });
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads 1", "event thread");
    es.emit("ready", {});
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads 0", "the list replaces the answered event");
    // Failed: the changes kept for it are dropped, so a later list does not replay them.
    failNext = true;
    es.emit("ready", {});
    await waitFor(() => root.querySelector(".banner.notice"), "failed load");
    es.emit("thread", { type: "thread", artifact_id: ID, thread: t("01JB") });
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads 1", "event after the failure");
    failNext = false;
    es.emit("ready", {});
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads 0", "the next list is not patched with dropped changes");
  });

  it("starts each pick with an empty composer and shows a failed post only in the banner", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async (url, init) => url.startsWith("/api/viewers/")
        ? new Response(JSON.stringify(viewer))
        : init?.method === "POST"
          ? new Response(JSON.stringify({ error: { code: "internal", message: "disk full" } }), { status: 500 })
          : new Response(JSON.stringify({ threads: [], next_cursor: null })));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    // A pick counts only in comment mode.
    viewerPick(frame, pick("p0", "Not in comment mode"));
    await new Promise(r => setTimeout(r, 30));
    expect(root.querySelector(".composer")).toBeNull();
    buttonNamed(root, "Comment").click();
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode");
    viewerPick(frame, pick("p1", "Quarterly goals"));
    let textarea = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    textarea.value = "first draft";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Post comment").click();
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.textContent).toContain("Could not post: 500 disk full");
    expect(root.querySelector(".composer .error")).toBeNull();
    expect(root.querySelector(".composer")!.textContent).not.toContain("disk full");
    buttonNamed(root, "Comment").click();
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode");
    viewerPick(frame, pick("p2", "Grow revenue"));
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("Grow revenue"), "second pick");
    textarea = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    expect(textarea.value).toBe("");
  });

  it("opens no composer for a pick the page forged: without a start, with a start outside the viewer's gesture or without an anchor, or beside another pending start; and tells the bridge each start it refused", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    // Each message keeps what had focus when it was posted.
    const posted: { type: string; pickId?: string; focused?: Element | null }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push({ ...m, focused: document.activeElement }); }) as typeof win.postMessage;
    const refused = () => posted.filter(m => m.type === "clax:pick-refused").map(m => m.pickId);
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const comment = buttonNamed(root, "Comment");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode");
    const settle = () => new Promise(r => setTimeout(r, 30));
    gestureIn(frame);
    fromFrame(win, pick("f1", "No start"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    gestureIn(frame, false);
    fromFrame(win, startOf(pick("f2", "No gesture")));
    fromFrame(win, pick("f2", "No gesture"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    gestureIn(frame);
    fromFrame(win, { type: "clax:pick-start", pickId: "f3" });
    fromFrame(win, pick("f3", "No anchor"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    expect(comment.getAttribute("aria-pressed")).toBe("true");
    expect(refused()).toEqual(["f2", "f3"]);
    // The page's forged start beside the bridge's real one: neither counts,
    // the composer the first opened closes, and comment mode comes back.
    gestureIn(frame);
    fromFrame(win, startOf(pick("real", "Real")));
    fromFrame(win, startOf(pick("forged", "Forged")));
    fromFrame(win, pick("forged", "Forged"));
    fromFrame(win, pick("real", "Real"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode back");
    expect(refused()).toEqual(["f2", "f3", "forged", "real"]);
    expect(posted.some(m => m.type === "clax:composer-ready")).toBe(false);
    // Beside a composer the viewer has typed in: that composer and its text stay.
    gestureIn(frame);
    fromFrame(win, startOf(pick("mine", "Mine")));
    const textarea = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "the composer");
    // Its textarea has focus: the bridge may render the clip now.
    await waitFor(() => posted.some(m => m.type === "clax:composer-ready" && m.pickId === "mine"), "composer ready");
    // It had focus before the message went.
    expect(posted.find(m => m.type === "clax:composer-ready")!.focused).toBe(textarea);
    expect(document.activeElement).toBe(textarea);
    textarea.value = "my words";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    gestureIn(frame);
    fromFrame(win, startOf(pick("other", "Other")));
    await settle();
    expect(root.querySelector(".composer-quote")!.textContent).toContain("Mine");
    expect(root.querySelector<HTMLTextAreaElement>(".composer textarea")!.value).toBe("my words");
    expect(refused().at(-1)).toBe("other");
    fromFrame(win, { ...pick("mine", "Mine"), clipError: "kept" });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: kept"), "its screenshot still comes");
    buttonNamed(root, "Cancel").click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode after cancel");
    // A start is used once: a second pick under its ID changes nothing.
    viewerPick(frame, { ...pick("ok", "Quarterly goals"), clipError: "first" });
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("Quarterly goals"), "the viewer's pick");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode again");
    fromFrame(win, { ...pick("ok", "Replayed"), clipError: "replayed" });
    await settle();
    expect(root.querySelector(".composer-quote")!.textContent).toContain("Quarterly goals");
    expect(root.querySelector(".composer")!.textContent).toContain("No screenshot: first");
  });

  it("opens the composer at the viewer's pick start, focused and taking the screenshot, and takes no text from the page", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const comment = buttonNamed(root, "Comment");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode");
    const settle = () => new Promise(r => setTimeout(r, 30));
    gestureIn(frame);
    fromFrame(win, startOf(pick("p1", "Goals")));
    const textarea = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "the composer");
    await waitFor(() => root.querySelector(".composer")!.textContent!.includes("Taking the screenshot…"), "the status line");
    expect(buttonNamed(root, "Post comment").disabled).toBe(true);
    expect(comment.getAttribute("aria-pressed")).toBe("false");
    await waitFor(() => document.activeElement === textarea, "the textarea focused");
    // Text the page posts goes nowhere.
    fromFrame(win, { type: "clax:keys", pickId: "p1", keys: ["@", "a", "g", "e", "n", "t"], done: true });
    await settle();
    expect(textarea.value).toBe("");
    // The screenshot arrives: the composer keeps its anchor; Post waits for text.
    fromFrame(win, pick("p1", "Ignored anchor"));
    await waitFor(() => !root.querySelector(".composer")!.textContent!.includes("Taking the screenshot…"), "the screenshot");
    expect(root.querySelector(".composer-quote")!.textContent).toContain("Goals");
    textarea.value = "Via";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Cancel").click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode back");
  });

  it("tells a custom-anchors page areas are off while a send is in flight, and a page area's composer waits for its screenshot with Post disabled, then says it never came", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const thread = { id: "tS", artifact_id: ID, version_n: 1, anchor: pick("x", "Goals").anchor, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "tS", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "note", created_at: "x" }] };
    let answerSend!: (r: Response) => void;
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { comments: { customAnchors: true } } } };
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : declared)),
      async (url, init) => url.startsWith("/api/viewers/")
        ? new Response(JSON.stringify(viewer))
        : url.endsWith("/send") && init?.method === "POST"
          ? new Promise<Response>(r => { answerSend = r; })
          : new Response(JSON.stringify({ threads: [thread], next_cursor: null })));
    const root = view.root;
    (await import("./view/composer-model")).captureWait.ms = 50;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; topic?: string; data?: { on?: boolean; canArea?: boolean }; ok?: boolean; value?: { opened?: boolean } }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    fromFrame(win, { type: "clax:call", id: "r1", ns: "comments", method: "register", args: [] });
    await waitFor(() => posted.some(m => m.type === "clax:call-result" && m.id === "r1"), "registered");
    buttonNamed(root, "Comment").click();
    const lastMode = () => posted.filter(m => m.type === "clax:event" && m.topic === "mode").at(-1)?.data;
    await waitFor(() => lastMode()?.on === true && lastMode()?.canArea === true, "areas on in comment mode");
    const card = await waitFor(() => root.querySelector('[data-thread="tS"]'), "the thread");
    buttonNamed(card, "Send to claude").click();
    await waitFor(() => lastMode()?.canArea === false, "areas off while the send is in flight");
    answerSend(new Response(JSON.stringify({ thread: { ...thread, sent_to_agent: true } })));
    await waitFor(() => lastMode()?.canArea === true, "areas on again");
    // A page area: the composer opens at once, waiting for its screenshot.
    gestureIn(frame);
    fromFrame(win, { type: "clax:call", id: "c1", ns: "comments", method: "compose", args: [{ anchor: "body > h2", dom: true, area: true, clipPending: true, version: 1 }] });
    await waitFor(() => posted.find(m => m.type === "clax:call-result" && m.id === "c1"), "compose answered");
    expect(posted.find(m => m.id === "c1")!.value).toMatchObject({ opened: true });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("Taking the screenshot…"), "capturing");
    const textarea = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    textarea.value = "look here";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));
    expect(buttonNamed(root, "Post comment").getAttribute("aria-disabled")).toBe("true");
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: it was not taken in time"), "the late note");
    await waitFor(() => postReady(root), "post enabled");
  });

  it("drops a pick's clip past the daemon's cap with the reason, says when a thread was posted without its screenshot, and clears that on a post that kept its clip", async () => {
    let posts = 0;
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async (url, init) => url.startsWith("/api/viewers/")
        ? new Response(JSON.stringify(viewer))
        : init?.method === "POST"
          ? new Response(JSON.stringify({ thread: { id: `01JX${++posts}`, artifact_id: ID, version_n: 1, anchor: pick("x", "q").anchor, status: "open", sent_to_agent: false, has_clip: posts > 1, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] }, ...(posts === 1 ? { clip_error: "clip is not a PNG" } : {}) }), { status: 201 })
          : new Response(JSON.stringify({ threads: [], next_cursor: null })));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    fromFrame(frame.contentWindow!, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    buttonNamed(root, "Comment").click();
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode");
    viewerPick(frame, { ...pick("big", "Photo"), clipPng: new ArrayBuffer(5 * 1024 * 1024 + 1) });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: the screenshot was too large to keep"), "the reason");
    const textarea = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    textarea.value = "look";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => root.querySelector(".banner.notice")?.textContent?.includes("Posted without its screenshot: clip is not a PNG"), "the notice");
    // The next post keeps its clip: that notice goes (jsdom has no object URLs for its preview).
    const urls = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
    const had = { create: urls.createObjectURL, revoke: urls.revokeObjectURL };
    urls.createObjectURL = () => "blob:clip";
    urls.revokeObjectURL = () => {};
    onTestFinished(() => { urls.createObjectURL = had.create; urls.revokeObjectURL = had.revoke; });
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode again");
    viewerPick(frame, { ...pick("small", "Chart"), clipPng: new Uint8Array([137, 80, 78, 71]).buffer });
    const t2 = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "second composer");
    t2.value = "and this";
    t2.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => posts === 2 && !root.querySelector(".banner.notice"), "the notice cleared");
  });

  it("takes a pick's screenshot only for the composer its start opened, once, and forgets a pending pick when comment mode comes back on or a page greets, so the viewer's next pick works", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const hello = () => fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    hello();
    const comment = buttonNamed(root, "Comment");
    const on = async () => { if (comment.getAttribute("aria-pressed") !== "true") comment.click(); await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode on"); };
    const settle = () => new Promise(r => setTimeout(r, 30));
    const start = (id: string, quote: string) => { gestureIn(frame); fromFrame(win, startOf(pick(id, quote))); };
    const capturing = () => !!root.querySelector(".composer")?.textContent?.includes("Taking the screenshot…");
    const composerFor = (quote: string) => waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes(quote) && capturing(), quote);
    // The start opens the composer; its pick brings the screenshot, once.
    await on();
    start("a1", "First");
    await composerFor("First");
    fromFrame(win, { ...pick("a1", "First"), clipError: "blank" });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: blank"), "the pick's clip error");
    fromFrame(win, { ...pick("a1", "First"), clipError: "again" });
    await settle();
    expect(root.querySelector(".composer")!.textContent).toContain("No screenshot: blank");
    // Comment mode back on while a pick is in flight: the old pick is moot,
    // the new one works.
    await on();
    start("b1", "Old");
    await composerFor("Old");
    await on();
    start("b2", "New after a toggle");
    await composerFor("New after a toggle");
    fromFrame(win, { ...pick("b1", "Old"), clipError: "old" });
    await settle();
    expect(capturing()).toBe(true);
    fromFrame(win, { ...pick("b2", "New after a toggle"), clipError: "new" });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: new"), "the new pick's clip");
    // A pick from before a new greeting does not count after it.
    await on();
    start("c1", "Before the greeting");
    await composerFor("Before the greeting");
    hello();
    fromFrame(win, { ...pick("c1", "Before the greeting"), clipError: "stale" });
    await settle();
    expect(capturing()).toBe(true);
    await on();
    viewerPick(frame, { ...pick("c2", "After the greeting"), clipError: "fresh" });
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("After the greeting") && root.querySelector(".composer")?.textContent?.includes("No screenshot: fresh"), "after the greeting");
  });

  it("turns comment mode back on when a pick's composer closes by Post, by a post sent to the agent, or by Cancel or Escape, and not while a failed post keeps it open", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    let posts = 0;
    const bodies: string[] = [];
    let sends = 0;
    const thread = (id: string, body: string) => ({ id, artifact_id: ID, version_n: 1, anchor: pick("x", "q").anchor, status: "open", sent_to_agent: body.includes("@agent"), has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body, created_at: "x" }] });
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async (url, init) => {
        if (url.startsWith("/api/viewers/")) return new Response(JSON.stringify(viewer));
        if (url.endsWith("/send") && init?.method === "POST") { sends++; return new Response(JSON.stringify({ thread: { ...thread("t1", "second"), sent_to_agent: true } })); }
        if (init?.method === "POST") {
          const body = String((init.body as FormData).get("body"));
          bodies.push(body);
          if (++posts === 1) return new Response(JSON.stringify({ error: { code: "internal", message: "disk full" } }), { status: 500 });
          return new Response(JSON.stringify({ thread: thread(`t${posts - 1}`, body) }), { status: 201 });
        }
        return new Response(JSON.stringify({ threads: [], next_cursor: null }));
      });
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const toFrame: { type: string; on?: boolean }[] = [];
    win.postMessage = ((m: (typeof toFrame)[number]) => { toFrame.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const comment = buttonNamed(root, "Comment");
    const pressed = () => comment.getAttribute("aria-pressed") === "true";
    const lastMode = () => toFrame.filter(m => m.type === "clax:comment-mode").at(-1)?.on;
    const settle = () => new Promise(r => setTimeout(r, 30));
    const type = async (text: string) => {
      const t = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
      t.value = text;
      t.dispatchEvent(new Event("input", { bubbles: true }));
      await waitFor(() => postReady(root), "post enabled");
    };
    comment.click();
    await waitFor(pressed, "comment mode");
    // Off while the composer is open, and still off after a failed post.
    viewerPick(frame, pick("p1", "Goals"));
    await type("first");
    expect(pressed()).toBe(false);
    // The frame hears of it from an effect, which runs after the render.
    await waitFor(() => lastMode() === false, "comment mode off in the frame");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => root.querySelector(".banner.notice"), "the failed post");
    await settle();
    expect(root.querySelector(".composer")).not.toBeNull();
    expect(pressed()).toBe(false);
    expect(lastMode()).toBe(false);
    // Cancel closes it: comment mode is back on, in the shell and the frame.
    buttonNamed(root, "Cancel").click();
    await waitFor(() => pressed() && lastMode() === true, "comment mode back after Cancel");
    // The next pick needs no press of Comment; Post closes it and mode is back on.
    viewerPick(frame, pick("p2", "Revenue"));
    await type("second");
    expect(pressed()).toBe(false);
    buttonNamed(root, "Post comment").click();
    await waitFor(() => !root.querySelector(".composer") && pressed() && lastMode() === true, "comment mode back after Post");
    // Sending the posted thread to the agent from its card leaves it on.
    // A pointer's click (detail 1): the composer's focus tainted the keyboard trail (`keyboardTrail`).
    buttonNamed(await waitFor(() => root.querySelector('[data-thread="t1"]'), "the posted thread"), "Send to claude").dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
    await waitFor(() => sends === 1, "the send");
    await settle();
    expect(pressed()).toBe(true);
    // A post that sends itself to the agent (`@agent`) closes it too.
    viewerPick(frame, pick("p3", "Costs"));
    await type("@agent third");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => !root.querySelector(".composer") && pressed(), "comment mode back after a post to the agent");
    expect(bodies).toEqual(["first", "second", "@agent third"]);
    // Escape in the composer cancels it: back on.
    viewerPick(frame, pick("p4", "Hiring"));
    const t4 = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    expect(pressed()).toBe(false);
    t4.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await waitFor(() => !root.querySelector(".composer") && pressed(), "comment mode back after Escape in the composer");
    // Escape with no composer open still ends comment mode, in the shell and from the page.
    root.querySelector("header")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await waitFor(() => !pressed() && lastMode() === false, "comment mode off on Escape");
    comment.click();
    await waitFor(pressed, "comment mode on");
    fromFrame(win, { type: "clax:cancel" });
    await waitFor(() => !pressed(), "comment mode off on the page's cancel");
  });

  it("leaves comment mode off when a composer closes that no pick in comment mode opened, or that the viewer turned mode on and off over", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tR", artifact_id: ID, version_n: 1, anchor: pick("x", "Goals").anchor, status: "open", sent_to_agent: true, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "tR", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "note", created_at: "x" }] };
    let replies = 0;
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { comments: {} } } };
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : declared)),
      async (url, init) => {
        if (url.startsWith("/api/viewers/")) return new Response(JSON.stringify(viewer));
        if (url.includes("/comments") && init?.method === "POST") { replies++; return new Response(JSON.stringify({ thread: t }), { status: 201 }); }
        return new Response(JSON.stringify({ threads: [t], next_cursor: null }));
      });
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; value?: { opened?: boolean } }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    const comment = buttonNamed(root, "Comment");
    const pressed = () => comment.getAttribute("aria-pressed") === "true";
    const settle = () => new Promise(r => setTimeout(r, 30));
    const pageOpens = async (id: string) => {
      gestureIn(frame);
      fromFrame(win, { type: "clax:call", id, ns: "comments", method: "openComposer", args: [{ anchor: pick("x", "Goals").anchor }] });
      await waitFor(() => posted.find(m => m.type === "clax:call-result" && m.id === id)?.value?.opened, `the page's composer ${id}`);
    };
    const cancelStaysOff = async () => {
      buttonNamed(root, "Cancel").click();
      await waitFor(() => !root.querySelector(".composer"), "composer closed");
      await settle();
      expect(pressed()).toBe(false);
    };
    // The page's openComposer, then Cancel: mode stays off.
    await pageOpens("o1");
    await cancelStaysOff();
    // A pick's empty composer that the page's openComposer replaced: closing it leaves mode off.
    comment.click();
    await waitFor(pressed, "comment mode");
    viewerPick(frame, pick("p1", "Picked"));
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("Picked"), "the pick's composer");
    await pageOpens("o2");
    await cancelStaysOff();
    // The viewer turned mode on and off again over a pick's composer: it stays off.
    comment.click();
    await waitFor(pressed, "comment mode");
    viewerPick(frame, pick("p2", "Picked again"));
    await waitFor(() => root.querySelector(".composer") && !pressed(), "the pick's composer");
    comment.click();
    await waitFor(pressed, "on over the composer");
    comment.click();
    await waitFor(() => !pressed(), "off over the composer");
    await cancelStaysOff();
    // A reply on a thread card does not turn it on.
    const card = await waitFor(() => root.querySelector('[data-thread="tR"]'), "the thread");
    const input = card.querySelector<HTMLInputElement>("input[aria-label=Reply]")!;
    input.value = "more";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    buttonNamed(card, "Reply").click();
    await waitFor(() => replies === 1, "the reply");
    await settle();
    expect(pressed()).toBe(false);
  });

  it("tells a custom-anchors page comment mode came back, with areas, when a pick's composer closes", async () => {
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { comments: { customAnchors: true } } } };
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : declared)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; topic?: string; data?: { on?: boolean; canArea?: boolean } }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    fromFrame(win, { type: "clax:call", id: "r1", ns: "comments", method: "register", args: [] });
    await waitFor(() => posted.some(m => m.type === "clax:call-result" && m.id === "r1"), "registered");
    const modes = () => posted.filter(m => m.type === "clax:event" && m.topic === "mode").map(m => m.data);
    buttonNamed(root, "Comment").click();
    await waitFor(() => modes().at(-1)?.on === true, "mode on");
    viewerPick(frame, pick("p1", "Goals"));
    await waitFor(() => modes().at(-1)?.on === false, "mode off while composing");
    buttonNamed(root, "Cancel").click();
    await waitFor(() => modes().at(-1)?.on === true, "mode back on");
    expect(modes()).toEqual([{ on: false, canArea: false }, { on: true, canArea: true }, { on: false, canArea: false }, { on: true, canArea: true }]);
  });

  it("sends Escape to the frame while commenting with the pointer over it, and leaves comment mode only when the page answers", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; key?: string; down?: boolean }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    const comment = buttonNamed(root, "Comment");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode");
    frame.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(posted.filter(m => m.type === "clax:key").at(-1)).toEqual({ type: "clax:key", key: "Escape", down: true });
    await new Promise(r => setTimeout(r, 30));
    // The page dropped a drag: comment mode stays on.
    expect(comment.getAttribute("aria-pressed")).toBe("true");
    fromFrame(win, { type: "clax:cancel" });
    await waitFor(() => comment.getAttribute("aria-pressed") === "false", "comment mode off on the page's answer");
    // Away from the frame, Escape ends comment mode in the shell.
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode again");
    root.querySelector("header")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    const keys = posted.filter(m => m.type === "clax:key").length;
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await waitFor(() => comment.getAttribute("aria-pressed") === "false", "comment mode off");
    expect(posted.filter(m => m.type === "clax:key")).toHaveLength(keys);
  });

  it("focuses the frame on the thread hovered in the list or selected, by its anchor handle, so its drawn area is outlined", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tZ", artifact_id: ID, version_n: 1, anchor: { kind: "area", selector: "main", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, area: { x: 0, y: 0, w: 0.5, h: 0.5 }, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "tZ", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "gap", created_at: "x" }] };
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string | null; anchors?: { id: string }[] }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    const card = await waitFor(() => root.querySelector('[data-thread="tZ"]'), "the card");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    // The frame knows the thread only by the opaque handle it was sent.
    const first = await waitFor(() => posted.filter(m => m.type === "clax:resolve-anchors").at(-1)?.anchors?.[0], "the resolve request");
    // Made on the version shown: the frame skips the area's fingerprint.
    expect((first as { sameVersion?: boolean }).sameVersion).toBe(true);
    const handle = first.id;
    expect(handle).not.toBe("tZ");
    const lastFocus = () => posted.filter(m => m.type === "clax:focus").at(-1);
    expect(lastFocus()).toEqual({ type: "clax:focus", id: null });
    card.dispatchEvent(new MouseEvent("mouseenter"));
    await waitFor(() => lastFocus()?.id === handle, "focus on the hovered card");
    card.dispatchEvent(new MouseEvent("mouseleave"));
    await waitFor(() => lastFocus()?.id === null, "focus cleared");
    card.querySelector<HTMLButtonElement>("button.card-head")!.click();
    await waitFor(() => lastFocus()?.id === handle, "focus on the selected thread");
    expect(posted.some(m => m.type === "clax:focus" && m.id === "tZ")).toBe(false);
  });

  it("tells the frame which threads were made on the version shown, to resolve and to scroll to", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = (id: string, n: number) => ({ id, artifact_id: ID, version_n: n, anchor: { ...pick("x", `Goals ${id}`).anchor }, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body: `note ${id}`, created_at: "x" }] });
    const two = { artifact: artifact(2).artifact, versions: [...artifact(1).versions, ...artifact(2).versions] };
    const view = await mountView(async () => new Response(JSON.stringify(two)),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t("tOld", 1), t("tNew", 2)], next_cursor: null } : viewer)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; anchors?: { id: string; sameVersion?: boolean }[]; anchor?: { quote: string | null }; sameVersion?: boolean }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    await waitFor(() => root.querySelector('[data-thread="tOld"]'), "threads");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" });
    const req = await waitFor(() => posted.filter(m => m.type === "clax:resolve-anchors").at(-1)?.anchors?.length === 2 && posted.filter(m => m.type === "clax:resolve-anchors").at(-1), "the resolve request");
    expect(req.anchors!.map(a => a.sameVersion)).toEqual([false, true]);
    for (const [id, same] of [["tOld", false], ["tNew", true]] as const) {
      root.querySelector<HTMLButtonElement>(`[data-thread="${id}"] button.card-head`)!.click();
      const scroll = await waitFor(() => posted.filter(m => m.type === "clax:scroll-to").at(-1)?.anchor?.quote === `Goals ${id}` && posted.filter(m => m.type === "clax:scroll-to").at(-1), `scroll to ${id}`);
      expect(scroll.sameVersion).toBe(same);
    }
  });

  it("forwards Option and, with it, Up and Down to the frame while commenting with the pointer over it", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; key?: string; down?: boolean }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    const keys = () => posted.filter(m => m.type === "clax:key").map(m => `${m.key}:${m.down}`);
    const press = (key: string, type = "keydown", altKey = false) => { const e = new KeyboardEvent(type, { key, altKey, bubbles: true, cancelable: true }); document.body.dispatchEvent(e); return e; };
    frame.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    press("Alt");
    expect(keys()).toEqual([]);
    buttonNamed(root, "Comment").click();
    await waitFor(() => posted.some(m => m.type === "clax:comment-mode"), "comment mode on");
    press("Alt");
    expect(press("ArrowUp", "keydown", true).defaultPrevented).toBe(true);
    press("ArrowDown", "keydown", true);
    expect(press("ArrowUp").defaultPrevented).toBe(false);
    press("Alt", "keyup");
    expect(keys()).toEqual(["Alt:true", "ArrowUp:true", "ArrowDown:true", "Alt:false"]);
    // The pointer left the frame: nothing more is forwarded.
    root.querySelector("header")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    press("Alt");
    expect(keys()).toHaveLength(4);
  });
});

function stubMedia(matches: Record<string, boolean>) {
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: matches[q] ?? false, media: q, addEventListener() {}, removeEventListener() {} }));
}

/** Whether Post posts when pressed: enabled, and not waiting for the screenshot. */
function postReady(root: Element): boolean {
  const b = buttonNamed(root, "Post comment");
  return !b.disabled && b.getAttribute("aria-disabled") !== "true";
}

/** A button's text without its `aria-hidden` parts (a keycap), as its accessible name reads. */
function nameOf(b: HTMLButtonElement): string {
  const c = b.cloneNode(true) as HTMLElement;
  for (const h of c.querySelectorAll("[aria-hidden=true]")) h.remove();
  return (c.textContent ?? "").trim();
}

function buttonNamed(root: Element, name: string | RegExp): HTMLButtonElement {
  const b = Array.from(root.querySelectorAll("button")).find(x => (typeof name === "string" ? nameOf(x) === name : name.test(nameOf(x))));
  if (!b) throw new Error(`no button ${name}`);
  return b;
}

/** A message from the content frame as a sandboxed (opaque-origin) bridge sends it. */
function fromFrame(win: Window, data: unknown) {
  dispatchTrusted(window, new MessageEvent("message", { data, origin: "null", source: win }));
}

/** The viewer's gesture in the content frame: transient activation, with
 * the pointer moved onto the frame and focus into it after the shell's own
 * control. */
function gestureIn(frame: HTMLIFrameElement, active = true) {
  Object.defineProperty(navigator, "userActivation", { value: { isActive: active }, configurable: true });
  const control = document.createElement("button");
  document.body.appendChild(control);
  gesture!.notePointerOver(control, 900, 10);
  gesture!.notePointerAt(900, 10);
  control.focus();
  gesture!.notePointerOver(frame, 100, 100);
  frame.focus();
  control.remove();
}

/** A pick as the bridge sends one for the viewer's click: its start, with the
 * anchor, while the frame holds the gesture, then the pick. */
function viewerPick(frame: HTMLIFrameElement, m: { pickId: string; [k: string]: unknown }) {
  gestureIn(frame);
  fromFrame(frame.contentWindow!, startOf(m));
  fromFrame(frame.contentWindow!, m);
}

/** The start the bridge sends for the pick `m`. */
function startOf(m: { pickId: string; [k: string]: unknown }) {
  return { type: "clax:pick-start", pickId: m.pickId, version: m.version, anchor: m.anchor };
}

function pick(pickId: string, quote: string) {
  return { type: "clax:pick", pickId, version: 1, anchor: { kind: "element", selector: "body > h2", quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" } };
}
