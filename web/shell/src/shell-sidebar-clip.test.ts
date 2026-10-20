import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import type { Thread, Viewer } from "./threads";
import ShellSidebar from "./ui/ShellSidebar.svelte";

const TID = "01J9T0000000000000000000AB";
const CLIP = `/api/artifacts/7q3k9mzx2b4t/threads/${TID}/clip`;
const at = "2026-10-09T10:00:00.000Z";
const thread: Thread = {
  id: TID, artifact_id: "7q3k9mzx2b4t", version_n: 1, status: "open", sent_to_agent: false, has_clip: true, clip_url: CLIP, created_at: at,
  resolved_at: null, resolved_by: null, feedback_state: null,
  anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
  comments: [{ id: "c1", thread_id: TID, author_kind: "viewer", author_name: "Mia", via_harness: null, body: "Here", created_at: at }],
};
const OWNER: Viewer = { public_id: "u_alex", display_name: "Alex", created_at: at, owner: true };

/** The artifact view's sidebar holding `thread`. */
function shell() {
  const s = { threads: [thread], resolved: {}, selected: null, file: "index.html", me: OWNER, data: { versions: [], artifact: { owner_harness: "claude" } }, working: [], commenting: false,
    agents: [], attention: null, decided: null, selection: { ids: [], anchor: null }, batchNote: "", batchBusy: false, sendTo: null };
  return mount(ShellSidebar, { ctl: { holds: () => true, shown: () => 1 } as never, s: s as never });
}
const settle = async () => { for (let i = 0; i < 8; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
const thumb = (root: Element) => root.querySelector<HTMLImageElement>(".clip-thumb img");

afterEach(() => {
  delete (window as { claxTransport?: unknown }).claxTransport;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

describe("a thread's clip in the artifact view's sidebar", () => {
  it("loads from its URL in a browser", () => {
    const m = shell();
    expect(thumb(m.root)?.getAttribute("src")).toBe(CLIP);
  });

  it("is fetched over the transport and shown by object URL in the Clax app", async () => {
    Object.defineProperty(window, "claxTransport", { configurable: true, value: Object.freeze({ embed: { transport: "host", shell: "https://app.clax.invalid", content: "https://{id}.c.clax.invalid" } }) });
    const fetch = vi.fn(async () => new Response(new Uint8Array([137, 80, 78, 71]), { headers: { "content-type": "image/png" } }));
    vi.stubGlobal("fetch", fetch);
    // jsdom has no URL.createObjectURL.
    Object.defineProperty(URL, "createObjectURL", { value: () => "blob:the-clip", configurable: true, writable: true });
    const m = shell();
    await settle();
    expect(fetch).toHaveBeenCalledWith(CLIP);
    expect(thumb(m.root)?.getAttribute("src")).toBe("blob:the-clip");
  });
});
