import { describe, it, expect, vi } from "vitest";
import { MOUNT_TIMEOUT_MS } from "./test/timeouts";
import { artifactViewHooks, waitFor, ID, FakeES, artifact, page, viewer, mountView, stubMedia, buttonNamed, fromFrame, gestureIn } from "./test/artifact-view";

describe("ArtifactView", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();

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
    (await waitFor(() => FakeES.last, "event stream")).emit("resync", {});
    await new Promise(r => setTimeout(r, 30));
    expect(root.querySelector("button.reload")).toBeNull();
    current = 4;
    (await waitFor(() => FakeES.last, "event stream")).emit("resync", {});
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

  it("opens the event stream only after the viewer lookup and the token request answered, with no token in its URL", async () => {
    let answerViewer!: () => void;
    let tokenAsked = false;
    // Not the owner: the owner's topics (`questions`, `inbox`) stay out.
    const view = await mountView(async url => {
      if (url === "/api/token") tokenAsked = true;
      if (/^\/api\/(inbox|questions)/.test(url)) return new Response("{}", { status: 403 });
      return new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1)));
    },
      url => url.includes("/threads")
        ? Promise.resolve(new Response(JSON.stringify({ threads: [], next_cursor: null })))
        : new Promise<Response>(r => { answerViewer = () => r(new Response(JSON.stringify(viewer))); }));
    const root = view.root;
    await waitFor(() => root.querySelector("iframe.frame"), "viewer");
    await new Promise(r => setTimeout(r, 30));
    expect(FakeES.last).toBeUndefined();
    answerViewer();
    const es = await waitFor(() => FakeES.last, "event stream");
    // The token request set the events cookie, which stands for the token.
    expect(tokenAsked).toBe(true);
    expect(es.topics).toEqual([`artifact:${ID}`, `presence:${ID}`, `working:${ID}`]);
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
    // Its head opens it in place; "Go to page" opens its page.
    card.querySelector<HTMLButtonElement>("button.card-head")!.click();
    await waitFor(() => card.querySelector('input[aria-label="Reply"]'), "the card opened in place");
    expect(location.pathname).toBe(`/a/${ID}`);
    card.querySelector<HTMLButtonElement>("button.go-page")!.click();
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
    // The token is served, so the view loads the question module: let that
    // load finish before the registry is reset, or the next view's imports
    // race a half-loaded copy of the modules they share.
    await import("./q");
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
    // The wait for the page runs on the shell's clock: each step moves it past.
    const pastTheWait = async () => {
      (globalThis as unknown as { claxTestClock: { advance(ms: number): void } }).claxTestClock.advance(wait.ms);
      await new Promise(r => setTimeout(r, 0));
    };
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    win.postMessage = (() => {}) as typeof win.postMessage;
    const open = async () => {
      const card = await waitFor(() => root.querySelector('[data-thread="tA"]'), "the card");
      card.querySelector<HTMLButtonElement>("button.go-page")!.click();
      await waitFor(() => location.pathname === `/a/${ID}/about.html`, "navigated");
    };
    // A fast Back abandons the jump without a notice.
    await open();
    history.back();
    await waitFor(() => location.pathname === `/a/${ID}`, "back on the index URL");
    await pastTheWait();
    expect(root.querySelector(".banner.notice")).toBeNull();
    // Another page greeting instead abandons it too.
    await open();
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await pastTheWait();
    expect(root.querySelector(".banner.notice")).toBeNull();
    // Nothing greets: the notice.
    await open();
    await pastTheWait();
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "failure banner");
    expect(banner.textContent).toContain("Could not open about.html");
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
});
