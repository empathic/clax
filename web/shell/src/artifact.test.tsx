import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "preact";

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
const artifact = (n: number, files: Record<string, unknown> = {}) => ({ artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "2026-09-28T11:00:00Z", current_version: n, pinned: false }, versions: [{ artifact_id: ID, n, label: null, created_at: "x", files }] });
const page = { content_type: "text/html", size: 1 };

const viewer = { viewer: { public_id: "u_0123456789abcdef012345", display_name: null, created_at: "x" } };

/** Answers the comment routes (no threads, an anonymous viewer) unless `comments` is given; everything else goes to `fetchImpl`. */
async function mount(fetchImpl: (url: string) => Promise<Response>, comments?: (url: string, init?: RequestInit) => Promise<Response>, file?: string) {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes("/threads") || url.startsWith("/api/viewers/")) {
      if (comments) return comments(url, init);
      return new Response(JSON.stringify(url.includes("/threads") ? { threads: [], next_cursor: null } : viewer));
    }
    return fetchImpl(url);
  }));
  sessionStorage.setItem("artifax.origin-ok", "0");
  const { default: ArtifactView } = await import("./artifact");
  const root = document.createElement("div");
  document.body.appendChild(root);
  render(<ArtifactView id={ID} pinnedVersion={null} file={file} />, root);
  return root;
}

describe("ArtifactView", () => {
  // `forgetViewer` comes from the fresh module registry, the same `threads`
  // module the test's `import("./artifact")` then loads.
  beforeEach(async () => { vi.resetModules(); (await import("./threads")).forgetViewer(); FakeES.last = undefined; });
  beforeEach(() => { history.replaceState(null, "", `/a/${ID}`); });
  afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); history.replaceState(null, "", "/"); });

  it("says not found only for a 404 status", async () => {
    const root = await mount(async () => new Response(JSON.stringify({ error: { message: "nope" } }), { status: 404 }));
    await waitFor(() => root.textContent?.includes("Artifact not found"), "not-found message");
  });

  it("shows the raw error for other failures, even when the text contains 404", async () => {
    const root = await mount(async () => new Response(JSON.stringify({ error: { message: "port 4040 or 404 busy" } }), { status: 500 }));
    await waitFor(() => root.textContent?.includes("port 4040"), "error message");
    expect(root.textContent).not.toContain("Artifact not found");
  });

  it("refetches on resync and shows the banner when a newer version exists", async () => {
    let current = 1;
    const root = await mount(async () => new Response(JSON.stringify(artifact(current))));
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    expect(root.querySelector(".banner")).toBeNull();
    (await waitFor(() => FakeES.last, "event stream")).emit("resync", { dropped: 3 });
    await new Promise(r => setTimeout(r, 30));
    expect(root.querySelector(".banner")).toBeNull();
    current = 4;
    (await waitFor(() => FakeES.last, "event stream")).emit("resync", { dropped: 3 });
    await waitFor(() => root.querySelector(".banner")?.textContent?.includes("v4 published"), "banner");
  });

  it("reloads the threads when the event stream (re)connects", async () => {
    let listed = 0;
    const t = { id: "01JA", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "01JA", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "made while the daemon restarted", created_at: "x" }] };
    const root = await mount(async () => new Response(JSON.stringify(artifact(1))),
      async url => url.includes("/threads")
        ? new Response(JSON.stringify({ threads: listed++ === 0 ? [] : [t], next_cursor: null }))
        : new Response(JSON.stringify(viewer)));
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    await waitFor(() => listed === 1, "initial thread load");
    expect(buttonNamed(root, /^Threads/).textContent).toBe("Threads (0)");
    (await waitFor(() => FakeES.last, "event stream")).emit("ready", {});
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads (1)", "thread from the reload");
  });

  it("opens the event stream only after the viewer lookup answered, with the owner shell's token", async () => {
    let answerViewer!: () => void;
    const root = await mount(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1))),
      url => url.includes("/threads")
        ? Promise.resolve(new Response(JSON.stringify({ threads: [], next_cursor: null })))
        : new Promise<Response>(r => { answerViewer = () => r(new Response(JSON.stringify(viewer))); }));
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
    const root = await mount(async () => new Response(JSON.stringify(artifact(1))),
      url => url.includes("/threads")
        ? new Promise<Response>(r => { answerList = () => r(new Response(JSON.stringify({ threads: [], next_cursor: null }))); })
        : Promise.resolve(new Response(JSON.stringify(viewer))));
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    (await waitFor(() => FakeES.last, "event stream")).emit("thread", { type: "thread", artifact_id: ID, thread: t });
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads (1)", "thread from the event");
    answerList();
    await new Promise(r => setTimeout(r, 30));
    expect(buttonNamed(root, /^Threads/).textContent).toBe("Threads (1)");
  });

  it("shows a failed thread load in the notice banner", async () => {
    const root = await mount(async () => new Response(JSON.stringify(artifact(1))),
      async url => url.includes("/threads")
        ? new Response(JSON.stringify({ error: { code: "internal", message: "db locked" } }), { status: 500 })
        : new Response(JSON.stringify(viewer)));
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.getAttribute("role")).toBe("alert");
    expect(banner.textContent).toContain("Could not load comments: 500 db locked");
  });

  it("says a failed name lookup could not load the name", async () => {
    const root = await mount(async () => new Response(JSON.stringify(artifact(1))),
      async url => url.includes("/threads")
        ? new Response(JSON.stringify({ threads: [], next_cursor: null }))
        : new Response(JSON.stringify({ error: { code: "forbidden_origin", message: "nope" } }), { status: 403 }));
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.textContent).toContain("Could not load your name: 403 nope");
  });

  it("puts \"Your name\" in the header when wide and in the Threads panel when narrow", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    let root = await mount(async () => new Response(JSON.stringify(artifact(1))));
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    expect(root.querySelectorAll('input[aria-label="Your name"]')).toHaveLength(1);
    expect(root.querySelector('.topbar input[aria-label="Your name"]')).not.toBeNull();
    render(null, root);
    document.body.replaceChildren();
    vi.resetModules();

    stubMedia({ "(min-width: 900px)": false, "(max-width: 480px)": true });
    root = await mount(async () => new Response(JSON.stringify(artifact(1))));
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    expect(root.querySelector('input[aria-label="Your name"]')).toBeNull();
    buttonNamed(root, /^Threads/).click();
    await waitFor(() => root.querySelector('aside.sidebar input[aria-label="Your name"]'), "name field in the Threads panel");
    expect(root.querySelectorAll('input[aria-label="Your name"]')).toHaveLength(1);
  });

  it("ignores a hello from another artifact or version, and welcomes the shown one", async () => {
    const root = await mount(async () => new Response(JSON.stringify(artifact(2))));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: unknown[] = [];
    win.postMessage = ((m: unknown) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
    fromFrame(win, { type: "artifax:hello", artifact: "9zzzzzzzzzzz", version: 2, file: "index.html" });
    await new Promise(r => setTimeout(r, 20));
    expect(posted.filter(m => (m as { type: string }).type === "artifax:welcome")).toHaveLength(0);
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 2, file: "index.html" });
    await waitFor(() => posted.some(m => (m as { type: string }).type === "artifax:welcome"), "welcome");
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
      fromFrame(win, { type: "artifax:hello", artifact: ID, version: 2, file: "index.html" });
      fromFrame(win, { type: "artifax:use", id: "early", name: "permissions" });
    });
    seen.observe(document.body, { subtree: true, childList: true });
    await mount(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(2))));
    await waitFor(() => posted.some(m => m.type === "artifax:welcome"), "welcome");
    await waitFor(() => posted.some(m => m.type === "artifax:use-result" && m.id === "early"), "the answer to the early request");
  });

  it("answers capability requests only after a hello for the shown artifact and version", async () => {
    const root = await mount(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(2))));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    const results = () => posted.filter(m => m.type === "artifax:use-result").map(m => m.id);
    await new Promise(r => setTimeout(r, 30));
    fromFrame(win, { type: "artifax:use", id: "before", name: "permissions" });
    await new Promise(r => setTimeout(r, 30));
    expect(results()).toEqual([]);
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 2, file: "index.html" });
    fromFrame(win, { type: "artifax:use", id: "matched", name: "permissions" });
    await waitFor(() => results().includes("matched"), "use answer after the hello");
    // The frame navigated to another document, which greets as something else.
    fromFrame(win, { type: "artifax:hello", artifact: "9zzzzzzzzzzz", version: 2, file: "index.html" });
    fromFrame(win, { type: "artifax:use", id: "foreign", name: "permissions" });
    fromFrame(win, { type: "artifax:call", id: "foreign-call", ns: "permissions", method: "state", args: [] });
    await new Promise(r => setTimeout(r, 30));
    expect(results()).toEqual(["matched"]);
    expect(posted.some(m => m.type === "artifax:call-result")).toBe(false);
  });

  it("follows the page the frame shows: resolves its threads only, and opens another page's thread on that page", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = (id: string, file: string) => ({ id, artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: `Goals ${id}`, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body: `note ${id}`, created_at: "x" }] });
    const root = await mount(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "about.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t("tI", "index.html"), t("tA", "about.html")], next_cursor: null } : viewer)));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const posted: { type: string; anchors?: { id: string }[]; anchor?: { file: string } }[] = [];
    // jsdom gives the frame a new window when it navigates; a browser keeps one WindowProxy.
    const tap = () => { const w = frame.contentWindow!; w.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof w.postMessage; return w; };
    let win = tap();
    const lastResolve = () => posted.filter(m => m.type === "artifax:resolve-anchors").at(-1)?.anchors?.map(a => a.id);
    await waitFor(() => root.querySelector('[data-thread="tA"]'), "threads in the sidebar");
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => lastResolve()?.join() === "tI", "resolution of the index's threads only");
    const card = root.querySelector('[data-thread="tA"]')!;
    expect(card.querySelector(".file-label")!.textContent).toBe("on about.html");
    card.querySelector<HTMLButtonElement>("button.card-head")!.click();
    // One history entry: the shell URL is pushed and the frame is moved in place.
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "the shell URL names about.html");
    expect(posted.some(m => m.type === "artifax:scroll-to")).toBe(false);
    win = tap();
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "about.html" });

    await waitFor(() => lastResolve()?.join() === "tA", "resolution of about.html's threads");
    const scroll = await waitFor(() => posted.find(m => m.type === "artifax:scroll-to"), "scroll to the thread once its page greeted");
    expect(scroll.anchor!.file).toBe("about.html");
    await waitFor(() => !root.querySelector('[data-thread="tA"] .file-label') && root.querySelector('[data-thread="tI"] .file-label'), "labels follow the page");
  });

  it("opens the frame on the URL's page, and says so when the version does not hold it", async () => {
    let root = await mount(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "docs/about.html": page }))), undefined, "docs/about.html");
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/1/docs/about.html`);
    render(null, root);
    document.body.replaceChildren();
    vi.resetModules();
    root = await mount(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))), undefined, "gone.html");
    const msg = await waitFor(() => root.querySelector(".stage .empty"), "not-found message");
    expect(msg.textContent).toContain("v1 has no page gone.html");
    expect(msg.querySelector("a")!.getAttribute("href")).toBe(`/a/${ID}`);
    expect(root.querySelector("iframe")).toBeNull();
  });

  it("opens the frame at the URL's fragment, and keeps the address bar's fragment in step with the frame's", async () => {
    history.replaceState(null, "", `/a/${ID}/about.html#docs%2Fcontract.md`);
    const root = await mount(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "about.html": page }))), undefined, "about.html");
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/1/about.html#docs%2Fcontract.md`);
    const win = frame.contentWindow!;
    win.postMessage = (() => {}) as typeof win.postMessage;
    const depth = history.length;
    const settle = () => new Promise(r => setTimeout(r, 20));
    fromFrame(win, { type: "artifax:hash", hash: "#early" });
    await settle();
    expect(location.hash).toBe("#docs%2Fcontract.md");
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "about.html" });
    fromFrame(win, { type: "artifax:hash", hash: "#crates%2Fa.rs" });
    await waitFor(() => location.hash === "#crates%2Fa.rs", "the frame's fragment in the address bar");
    expect([location.pathname, history.length]).toEqual([`/a/${ID}/about.html`, depth]);
    for (const bad of ["no-hash", 42, `#${"x".repeat(512)}`]) fromFrame(win, { type: "artifax:hash", hash: bad });
    await settle();
    expect(location.hash).toBe("#crates%2Fa.rs");
    fromFrame(win, { type: "artifax:hash", hash: "" });
    await waitFor(() => location.hash === "", "no fragment");
    expect(location.pathname).toBe(`/a/${ID}/about.html`);
  });

  it("puts the page the frame greets from in the address bar, and ignores a page the version does not hold", async () => {
    const root = await mount(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1, { "index.html": page, "about.html": page }))));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
    await new Promise(r => setTimeout(r, 20));
    expect(location.pathname).toBe(`/a/${ID}`);
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "about.html" });
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "the about page's URL");
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => location.pathname === `/a/${ID}`, "back on the index's URL");
    // A page the shown version does not hold: no address change, and no answers.
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "forged.html" });
    fromFrame(win, { type: "artifax:use", id: "forged", name: "permissions" });
    await new Promise(r => setTimeout(r, 30));
    expect(location.pathname).toBe(`/a/${ID}`);
    expect(posted.some(m => m.type === "artifax:use-result" && m.id === "forged")).toBe(false);
  });

  it("drops the pins when the frame loads a document that never greets", async () => {
    const t = { id: "tI", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] };
    const root = await mount(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    win.postMessage = (() => {}) as typeof win.postMessage;
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads (1)", "thread loaded");
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
    fromFrame(win, { type: "artifax:anchors", requestId: "r1", results: [{ id: "tI", found: true, method: "exact", rect: { x: 10, y: 40, w: 100, h: 20 } }] });
    await waitFor(() => root.querySelector("button.thread-pin"), "the pin");
    frame.dispatchEvent(new Event("load"));
    expect(root.querySelector("button.thread-pin")).not.toBeNull();
    frame.dispatchEvent(new Event("load"));
    await waitFor(() => !root.querySelector("button.thread-pin"), "no pin over a document without the bridge");
  });

  it("lists a thread on a page the version does not hold as Detached, and opening it stays put", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tG", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Gone", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "gone.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] };
    const root = await mount(async () => new Response(JSON.stringify(artifact(1, { "index.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const card = await waitFor(() => root.querySelector('.section-detached [data-thread="tG"]'), "the detached card");
    card.querySelector<HTMLButtonElement>("button.card-head")!.click();
    await new Promise(r => setTimeout(r, 30));
    expect(frame.getAttribute("src")).toBe(`/c/${ID}/v/1/`);
    expect(location.pathname).toBe(`/a/${ID}`);
  });

  it("follows a link the page handed over as one history entry per greeting page, with its fragment", async () => {
    const root = await mount(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1, { "index.html": page, "about.html": page, "doc.pdf": { content_type: "application/pdf", size: 1 } }))));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const posted: { type: string; id?: string }[] = [];
    // jsdom gives the frame a new window when its src changes; a browser keeps one WindowProxy.
    const tap = () => { const w = frame.contentWindow!; w.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof w.postMessage; return w; };
    let win = tap();
    const settle = () => new Promise(r => setTimeout(r, 20));
    fromFrame(win, { type: "artifax:navigate", file: "about.html" });
    await settle();
    expect(location.pathname).toBe(`/a/${ID}`);
    const depth = history.length;
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
    fromFrame(win, { type: "artifax:navigate", file: "about.html", hash: "#team" });
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "the about page's URL");
    expect(location.hash).toBe("#team");
    // The outgoing document is no longer answered: a burst of links, a capability request.
    fromFrame(win, { type: "artifax:navigate", file: "index.html" });
    fromFrame(win, { type: "artifax:use", id: "stale", name: "permissions" });
    await settle();
    expect([location.pathname, history.length]).toEqual([`/a/${ID}/about.html`, depth + 1]);
    expect(posted.some(m => m.id === "stale")).toBe(false);
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "about.html" });
    await settle();
    expect(location.hash).toBe("#team");
    // Not an HTML page: the frame loads it in place, the URL stays.
    fromFrame(win, { type: "artifax:navigate", file: "doc.pdf" });
    await waitFor(() => frame.getAttribute("src") === `/c/${ID}/v/1/doc.pdf`, "the PDF in the frame");
    expect(location.pathname).toBe(`/a/${ID}/about.html`);
    win = tap();
    // Malformed requests are ignored: a non-string, a path outside the version, a missing page.
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "about.html" });
    for (const bad of [{ file: 42 }, { file: "../index.html" }, { file: "missing.html" }]) fromFrame(win, { type: "artifax:navigate", ...bad });
    await settle();
    expect([location.pathname, history.length]).toEqual([`/a/${ID}/about.html`, depth + 1]);
    // A fragment that is not one, or is too long, is dropped.
    fromFrame(win, { type: "artifax:navigate", file: "index.html", hash: "team" });
    await waitFor(() => location.pathname === `/a/${ID}`, "the index");
    expect(location.hash).toBe("");
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
    fromFrame(win, { type: "artifax:navigate", file: "about.html", hash: `#${"x".repeat(512)}` });
    await waitFor(() => location.pathname === `/a/${ID}/about.html`, "about again");
    expect(location.hash).toBe("");
  });

  it("says so when the page of an opened thread never greets, and not when the viewer moved on", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tA", artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Team", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "about.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] };
    const root = await mount(async () => new Response(JSON.stringify(artifact(1, { "index.html": page, "about.html": page }))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
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
      fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1, file: "index.html" });
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
    const root = await mount(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(2))));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push(m); }) as typeof win.postMessage;
    const results = () => posted.filter(m => m.type === "artifax:use-result").map(m => m.id);
    await new Promise(r => setTimeout(r, 30));
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 2, file: "index.html" });
    frame.dispatchEvent(new Event("load"));
    fromFrame(win, { type: "artifax:use", id: "greeted", name: "permissions" });
    await waitFor(() => results().includes("greeted"), "a load right after the hello keeps the gate open");
    // The frame navigated to a document without a bridge: it loads without greeting.
    frame.dispatchEvent(new Event("load"));
    fromFrame(win, { type: "artifax:use", id: "silent", name: "permissions" });
    await new Promise(r => setTimeout(r, 30));
    expect(results()).toEqual(["greeted"]);
  });

  it("drops the thread changes it kept once the latest list answered or failed", async () => {
    let lists = 0;
    let failNext = false;
    const t = (id: string) => ({ id, artifact_id: ID, version_n: 1, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] });
    const root = await mount(async () => new Response(JSON.stringify(artifact(1))),
      async url => {
        if (!url.includes("/threads")) return new Response(JSON.stringify(viewer));
        lists++;
        if (failNext) return new Response(JSON.stringify({ error: { code: "internal", message: "db locked" } }), { status: 500 });
        return new Response(JSON.stringify({ threads: [], next_cursor: null }));
      });
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    const es = await waitFor(() => FakeES.last, "event stream");
    await waitFor(() => lists >= 1, "initial list");
    await new Promise(r => setTimeout(r, 20));
    // Answered: an event now is not replayed onto the next list, which no longer has it.
    es.emit("thread", { type: "thread", artifact_id: ID, thread: t("01JA") });
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads (1)", "event thread");
    es.emit("ready", {});
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads (0)", "the list replaces the answered event");
    // Failed: the changes kept for it are dropped, so a later list does not replay them.
    failNext = true;
    es.emit("ready", {});
    await waitFor(() => root.querySelector(".banner.notice"), "failed load");
    es.emit("thread", { type: "thread", artifact_id: ID, thread: t("01JB") });
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads (1)", "event after the failure");
    failNext = false;
    es.emit("ready", {});
    await waitFor(() => buttonNamed(root, /^Threads/).textContent === "Threads (0)", "the next list is not patched with dropped changes");
  });

  it("starts each pick with an empty composer and shows a failed post only in the banner", async () => {
    const root = await mount(async () => new Response(JSON.stringify(artifact(1))),
      async (url, init) => url.startsWith("/api/viewers/")
        ? new Response(JSON.stringify(viewer))
        : init?.method === "POST"
          ? new Response(JSON.stringify({ error: { code: "internal", message: "disk full" } }), { status: 500 })
          : new Response(JSON.stringify({ threads: [], next_cursor: null })));
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    fromFrame(win, pick("p1", "Quarterly goals"));
    let textarea = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    textarea.value = "first draft";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => !buttonNamed(root, "Post comment").disabled, "post enabled");
    buttonNamed(root, "Post comment").click();
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.textContent).toContain("Could not post: 500 disk full");
    expect(root.querySelector(".composer .error")).toBeNull();
    expect(root.querySelector(".composer")!.textContent).not.toContain("disk full");
    fromFrame(win, pick("p2", "Grow revenue"));
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("Grow revenue"), "second pick");
    textarea = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    expect(textarea.value).toBe("");
  });
});

function stubMedia(matches: Record<string, boolean>) {
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: matches[q] ?? false, media: q, addEventListener() {}, removeEventListener() {} }));
}

function buttonNamed(root: Element, name: string | RegExp): HTMLButtonElement {
  const b = Array.from(root.querySelectorAll("button")).find(x => (typeof name === "string" ? x.textContent === name : name.test(x.textContent ?? "")));
  if (!b) throw new Error(`no button ${name}`);
  return b;
}

/** A message from the content frame as a sandboxed (opaque-origin) bridge sends it. */
function fromFrame(win: Window, data: unknown) {
  window.dispatchEvent(new MessageEvent("message", { data, origin: "null", source: win }));
}

function pick(pickId: string, quote: string) {
  return { type: "artifax:pick", pickId, version: 1, anchor: { kind: "element", selector: "body > h2", quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" } };
}
