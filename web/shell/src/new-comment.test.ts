import { afterEach, describe, expect, it, vi } from "vitest";
import NewComment from "./ui/NewComment.svelte";
import Sidebar from "./ui/Sidebar.svelte";
import { flush, mount } from "./test/svelte";
import type { Thread } from "./threads";
import { WHOLE_LABEL, type WholeHost, postWhole, wholeAnchor } from "./view/whole-comment";
import { whereLabel } from "./view/presence-model";
import type { ViewState } from "./view/artifact-controller";

const base = { artifact_id: "7q3k9mzx2b4t", version_n: 1, has_clip: false, clip_url: null, created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null, sent_to_agent: false };
const whole = (id: string): Thread => ({ ...base, id, anchor: wholeAnchor(), status: "open", comments: [{ id: "c", thread_id: id, author_kind: "viewer", author_name: "Alex", via_harness: null, body: "Warmer colours", created_at: base.created_at }] });
/** A view to post through, recording what posting did to it. */
function fakeHost() {
  const h = { threads: [] as Thread[], notice: null as string | null, selected: null as string | null };
  const host: WholeHost = {
    id: "7q3k9mzx2b4t",
    changeThreads: f => { h.threads = f(h.threads); },
    set: p => { h.selected = p.selected; },
    setNotice: u => { h.notice = typeof u === "function" ? u(h.notice) : u; },
  };
  return { h, host };
}
const created = () => vi.fn(async (_url: string, init: RequestInit) => new Response(JSON.stringify({ thread: { ...whole("new"), comments: [{ ...whole("new").comments[0], body: (init.body as FormData).get("body") as string }] } }), { status: 201 }));

afterEach(() => { vi.unstubAllGlobals(); document.body.innerHTML = ""; });

describe("whole-artifact anchors", () => {
  it("are on no page and labelled the whole artifact where the viewer is looking", () => {
    expect(wholeAnchor()).toMatchObject({ kind: "artifact", selector: null, quote: null, file: "" });
    expect(whereLabel({ threads: [whole("w")], selected: "w", draft: null } as unknown as ViewState)).toBe(WHOLE_LABEL);
  });

  it("post with the artifact anchor on the version given, then join the view selected; a failure shows, rethrows, and a later post clears it", async () => {
    const bodies: FormData[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => {
      bodies.push(init.body as FormData);
      return new Response(JSON.stringify({ thread: whole("new") }), { status: 201 });
    }));
    const { h, host } = fakeHost();
    const t = await postWhole(host, 2, "Warmer colours");
    expect(t.id).toBe("new");
    expect(JSON.parse(bodies[0].get("anchor") as string)).toEqual(wholeAnchor());
    expect(bodies[0].get("version")).toBe("2");
    expect(bodies[0].get("body")).toBe("Warmer colours");
    expect(bodies[0].get("clip")).toBeNull();
    expect(h.threads.map(x => x.id)).toEqual(["new"]);
    expect(h.selected).toBe("new");
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "invalid_anchor", message: "no" } }), { status: 400 })));
    await expect(postWhole(host, 2, "x")).rejects.toThrow();
    expect(h.notice).toMatch(/^Could not post: .*no$/);
    vi.stubGlobal("fetch", created());
    await postWhole(host, 2, "again");
    expect(h.notice).toBeNull();
  });
});

describe("a comment refused while comments are off", () => {
  it("says comments are off, and tells the view before rethrowing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "comments_off", message: "comments are off on this artifact" } }), { status: 403 })));
    const { h, host } = fakeHost();
    const seen: unknown[] = [];
    await expect(postWhole({ ...host, refused: e => { seen.push(e); throw e; } }, 1, "x")).rejects.toThrow();
    expect(h.notice).toBe("Could not post: comments are off for this page");
    expect(seen).toHaveLength(1);
  });
});

describe("NewComment", () => {
  it("opens the usual composer in place, posts the text on the version shown, and closes; Cancel closes without posting", async () => {
    const fetch = created();
    vi.stubGlobal("fetch", fetch);
    const { h, host } = fakeHost();
    const v = mount(NewComment, { version: 2, host });
    const open = () => flush(() => v.root.querySelector<HTMLButtonElement>(".new-comment-open")!.click());
    expect(v.root.querySelector(".composer")).toBeNull();
    open();
    const form = v.root.querySelector(".new-comment .composer")!;
    expect(form.querySelector(".composer-quote")!.textContent).toBe(WHOLE_LABEL);
    expect(form.querySelector(".file-label")).toBeNull();
    await vi.waitFor(() => expect(form.textContent).toContain("No screenshot: it is about the whole artifact"));
    const ta = form.querySelector("textarea")!;
    expect(document.activeElement).toBe(ta);
    flush(() => { ta.value = "Make the colours warmer"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => form.querySelector<HTMLButtonElement>("button[type=submit]")!.click());
    await vi.waitFor(() => expect(h.threads.map(t => t.comments[0].body)).toEqual(["Make the colours warmer"]));
    const sent = fetch.mock.calls[0][1].body as FormData;
    expect([sent.get("version"), JSON.parse(sent.get("anchor") as string).kind]).toEqual(["2", "artifact"]);
    await vi.waitFor(() => expect(v.root.querySelector(".composer")).toBeNull());
    open();
    flush(() => [...v.root.querySelectorAll("button")].find(b => b.textContent === "Cancel")!.click());
    expect(v.root.querySelector(".composer")).toBeNull();
    expect(fetch).toHaveBeenCalledTimes(1);
    v.unmount();
  });

  it("keeps the composer and its text when the post fails", async () => {
    const fetch = vi.fn(async () => { throw new Error("offline"); });
    vi.stubGlobal("fetch", fetch);
    const { h, host } = fakeHost();
    const v = mount(NewComment, { version: 1, host });
    flush(() => v.root.querySelector<HTMLButtonElement>(".new-comment-open")!.click());
    const ta = v.root.querySelector("textarea")!;
    flush(() => { ta.value = "Warmer"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => v.root.querySelector<HTMLButtonElement>("button[type=submit]")!.click());
    await vi.waitFor(() => expect(h.notice).toBe("Could not post: offline"));
    flush();
    expect(v.root.querySelector("textarea")!.value).toBe("Warmer");
    v.unmount();
  });

  it("shows in the panel below the questions and above the threads only when the panel composes; whole-artifact threads lead Open with no number", async () => {
    const props = { versions: [], shown: 1, agent: "claude", threads: [whole("w")], resolved: {}, selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() };
    const plain = mount(Sidebar, props);
    await Promise.resolve();
    flush();
    expect(plain.root.querySelector(".new-comment")).toBeNull();
    plain.unmount();
    const v = mount(Sidebar, { ...props, compose: { version: 1, host: fakeHost().host } });
    await vi.waitFor(() => expect(v.root.querySelector(".new-comment")).not.toBeNull());
    const aside = v.root.querySelector(".sidebar")!;
    expect(aside.firstElementChild!.classList.contains("questions-slot")).toBe(true);
    expect(v.root.querySelector(".new-comment")!.compareDocumentPosition(v.root.querySelector(".section-open")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const card = v.root.querySelector(".section-open .thread-card")!;
    expect(card.querySelector(".anchor-label")!.textContent).toBe(WHOLE_LABEL);
    expect(card.querySelector(".thread-num")).toBeNull();
    expect(card.querySelector(".file-label")).toBeNull();
    expect(v.root.querySelectorAll(".section-detached .thread-card")).toHaveLength(0);
    v.unmount();
  });

  it("names the box in the empty Open section only when the panel has it", () => {
    const props = { versions: [], shown: 1, agent: "claude", threads: [] as Thread[], resolved: {}, selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() };
    const plain = mount(Sidebar, props);
    expect(plain.root.querySelector(".empty-open")!.textContent).toBe("Nothing open. Press C and click anything to comment on it.");
    plain.unmount();
    const v = mount(Sidebar, { ...props, compose: { version: 1, host: fakeHost().host } });
    expect(v.root.querySelector(".empty-open")!.textContent).toBe("Nothing open. Press C and click anything to comment on it, or use New comment for the whole artifact.");
    v.unmount();
  });
});
