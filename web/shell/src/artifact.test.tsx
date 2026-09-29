import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "preact";

class FakeES {
  static last: FakeES;
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
const artifact = (n: number) => ({ artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "2026-09-28T11:00:00Z", current_version: n, pinned: false }, versions: [{ artifact_id: ID, n, label: null, created_at: "x", files: {} }] });

const viewer = { viewer: { id: "v", display_name: null, created_at: "x" } };

/** Answers the comment routes (no threads, an anonymous viewer) unless `comments` is given; everything else goes to `fetchImpl`. */
async function mount(fetchImpl: () => Promise<Response>, comments?: (url: string, init?: RequestInit) => Promise<Response>) {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes("/threads") || url.startsWith("/api/viewers/")) {
      if (comments) return comments(url, init);
      return new Response(JSON.stringify(url.includes("/threads") ? { threads: [], next_cursor: null } : viewer));
    }
    return fetchImpl();
  }));
  sessionStorage.setItem("artifax.origin-ok", "0");
  const { default: ArtifactView } = await import("./artifact");
  const root = document.createElement("div");
  document.body.appendChild(root);
  render(<ArtifactView id={ID} pinnedVersion={null} />, root);
  return root;
}

describe("ArtifactView", () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); });

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
    FakeES.last.emit("resync", { dropped: 3 });
    await new Promise(r => setTimeout(r, 30));
    expect(root.querySelector(".banner")).toBeNull();
    current = 4;
    FakeES.last.emit("resync", { dropped: 3 });
    await waitFor(() => root.querySelector(".banner")?.textContent?.includes("v4 published"), "banner");
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
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 1 });
    fromFrame(win, { type: "artifax:hello", artifact: "9zzzzzzzzzzz", version: 2 });
    await new Promise(r => setTimeout(r, 20));
    expect(posted.filter(m => (m as { type: string }).type === "artifax:welcome")).toHaveLength(0);
    fromFrame(win, { type: "artifax:hello", artifact: ID, version: 2 });
    await waitFor(() => posted.some(m => (m as { type: string }).type === "artifax:welcome"), "welcome");
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
  return { type: "artifax:pick", pickId, version: 1, anchor: { kind: "element", selector: "body > h2", quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null } };
}
