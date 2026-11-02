// A linked thread's card (ThreadCard through the sidebar): what it says of
// an up-send not sent, and Retry.
import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import type { Comment, Thread } from "./threads";
import Sidebar from "./ui/Sidebar.svelte";

const TID = "01J9T0000000000000000000AA";
const at = "2026-10-09T10:00:00.000Z";
const link = { remote_id: "r_1", remote: "clax.dev", remote_origin: "https://clax.dev", url: "https://clax.dev/a/abc", origin: "remote" as const,
  remote_version: 1, twin: true, unlinked: false, state: null, error: null };
const comment = (id: string, o: Partial<Comment> = {}): Comment => ({ id, thread_id: TID, author_kind: "viewer", author_name: "Mia", via_harness: null, body: `body ${id}`, created_at: at, ...o });
const notSent = { origin: "local" as const, remote: "clax.dev", author_kind: null, author_name: null, state: "failed" as const };
const thread = (o: Partial<Thread> = {}): Thread => ({
  id: TID, artifact_id: "7q3k9mzx2b4t", version_n: 1, has_clip: false, clip_url: null, created_at: at, resolved_at: null, resolved_by: null, feedback_state: null,
  anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
  status: "open", sent_to_agent: false, comments: [comment("c1"), comment("c2", { linked: notSent })], linked: link, ...o,
});
const render = (threads: Thread[], onSelect = vi.fn()) => mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads, resolved: {}, now: new Date("2026-10-09T10:05:00.000Z"),
  selected: null, onSelect, onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
const text = (root: Element) => [...root.querySelectorAll(".thread-card .link-state > span, .thread-card .st.link > span")].map(e => e.textContent);

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("a linked thread's card", () => {
  it("says a reply not sent once, with Retry beside it", () => {
    const m = render([thread()]);
    expect(text(m.root)).toEqual(["Not sent: clax.dev unreachable"]);
    expect(m.root.querySelectorAll(".thread-card button.retry")).toHaveLength(1);
    expect(m.root.querySelector(".thread-card .link-state button.retry")).not.toBeNull();
  });

  it("reads as sending again once Retry is taken, until a newer view of the thread comes", async () => {
    const fetch = vi.fn(async (_url: string, _init?: RequestInit) => new Response(JSON.stringify({ retried: 1 }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetch);
    const onSelect = vi.fn();
    const m = render([thread()], onSelect);
    m.root.querySelector<HTMLButtonElement>(".thread-card button.retry")!.click();
    await vi.waitFor(() => { flush(); expect(text(m.root)).toEqual(["Sending to clax.dev…"]); });
    expect(fetch.mock.calls.map(c => String(c[0]))).toContain("/api/artifacts/7q3k9mzx2b4t/remotes/r_1/retry");
    expect(m.root.querySelector(".thread-card button.retry")).toBeNull();
    expect(onSelect).not.toHaveBeenCalled();
    // A newer view saying it was not sent again shows Retry again.
    m.update({ versions: [], shown: 1, agent: "claude", threads: [thread()], resolved: {}, now: new Date("2026-10-09T10:05:00.000Z"),
      selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
    expect(text(m.root)).toEqual(["Not sent: clax.dev unreachable"]);
    expect(m.root.querySelector(".thread-card button.retry")).not.toBeNull();
  });

  it("says why a Retry failed on its line, keeping Retry, until a newer view of the thread comes", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => String(url).endsWith("/retry")
      ? new Response(JSON.stringify({ error: { code: "forbidden", message: "retrying is the owner's" } }), { status: 403, headers: { "content-type": "application/json" } })
      : new Response(JSON.stringify({ token: "t" }), { status: 200, headers: { "content-type": "application/json" } })));
    const props = (threads: Thread[]) => ({ versions: [], shown: 1, agent: "claude", threads, resolved: {}, now: new Date("2026-10-09T10:05:00.000Z"),
      selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
    const m = render([thread()]);
    m.root.querySelector<HTMLButtonElement>(".thread-card button.retry")!.click();
    const err = () => m.root.querySelector(".thread-card .link-state .retry-err")?.textContent;
    await vi.waitFor(() => { flush(); expect(err()).toBe("Retry failed: retrying is the owner's"); });
    expect(text(m.root)).toEqual(["Not sent: clax.dev unreachable", "Retry failed: retrying is the owner's"]);
    expect(m.root.querySelector<HTMLButtonElement>(".thread-card button.retry")!.disabled).toBe(false);
    m.update(props([thread()]));
    expect(err()).toBeUndefined();
  });
});
