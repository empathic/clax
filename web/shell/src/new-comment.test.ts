import { afterEach, describe, expect, it, vi } from "vitest";
import NewComment from "./ui/NewComment.svelte";
import Sidebar from "./ui/Sidebar.svelte";
import { flush, mount } from "./test/svelte";
import type { Thread } from "./threads";
import { WHOLE_LABEL, postWhole, threadLabel, wholeAnchor, wholeDraft } from "./view/whole-comment";
import { whereLabel } from "./view/presence-model";
import type { ViewState } from "./view/artifact-controller";

const base = { artifact_id: "7q3k9mzx2b4t", version_n: 1, has_clip: false, clip_url: null, created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null, sent_to_agent: false };
const whole = (id: string): Thread => ({ ...base, id, anchor: wholeAnchor(), status: "open", comments: [{ id: "c", thread_id: id, author_kind: "viewer", author_name: "Alex", via_harness: null, body: "Warmer colours", created_at: base.created_at }] });
const element = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };

afterEach(() => { vi.unstubAllGlobals(); document.body.innerHTML = ""; });

describe("whole-artifact anchors", () => {
  it("are on no page, labelled the whole artifact, and composed without a screenshot", () => {
    expect(wholeAnchor()).toMatchObject({ kind: "artifact", selector: null, quote: null, file: "" });
    expect(threadLabel(wholeAnchor())).toBe(WHOLE_LABEL);
    expect(threadLabel(element)).toBe("«Goals»");
    const d = wholeDraft(3, "p");
    expect(d).toMatchObject({ version: 3, clip: null, label: WHOLE_LABEL, anchor: { kind: "artifact" } });
    expect(whereLabel({ threads: [whole("w")], selected: "w", draft: null } as unknown as ViewState)).toBe(WHOLE_LABEL);
  });

  it("post with the artifact anchor on the version given, then join the view selected; a failure shows and rethrows", async () => {
    const bodies: FormData[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => {
      bodies.push(init.body as FormData);
      return new Response(JSON.stringify({ thread: whole("new") }), { status: 201 });
    }));
    let threads: Thread[] = [];
    let notice: string | null = null;
    const host = { id: "7q3k9mzx2b4t", changeThreads: vi.fn((f: (t: Thread[]) => Thread[]) => { threads = f(threads); }), set: vi.fn(), setNotice: (u: string | null | ((p: string | null) => string | null)) => { notice = typeof u === "function" ? u(notice) : u; } };
    const t = await postWhole(host, 2, "Warmer colours");
    expect(t.id).toBe("new");
    expect(JSON.parse(bodies[0].get("anchor") as string)).toEqual(wholeAnchor());
    expect(bodies[0].get("version")).toBe("2");
    expect(bodies[0].get("body")).toBe("Warmer colours");
    expect(bodies[0].get("clip")).toBeNull();
    expect(threads.map(x => x.id)).toEqual(["new"]);
    expect(host.set).toHaveBeenCalledWith({ selected: "new" });
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "invalid_anchor", message: "no" } }), { status: 400 })));
    await expect(postWhole(host, 2, "x")).rejects.toThrow();
    expect(notice).toContain("Could not post");
  });
});

describe("NewComment", () => {
  it("opens the usual composer in place, posts the text, and closes; Cancel closes without posting", async () => {
    const onPost = vi.fn(async () => {});
    const v = mount(NewComment, { version: 2, onPost });
    const open = () => flush(() => v.root.querySelector<HTMLButtonElement>(".new-comment-open")!.click());
    expect(v.root.querySelector(".composer")).toBeNull();
    open();
    const form = v.root.querySelector(".new-comment .composer")!;
    expect(form.querySelector(".composer-quote")!.textContent).toBe(WHOLE_LABEL);
    expect(form.querySelector(".file-label")).toBeNull();
    const ta = form.querySelector("textarea")!;
    expect(document.activeElement).toBe(ta);
    flush(() => { ta.value = "Make the colours warmer"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => form.querySelector<HTMLButtonElement>("button[type=submit]")!.click());
    await vi.waitFor(() => expect(onPost).toHaveBeenCalledWith("Make the colours warmer"));
    await vi.waitFor(() => expect(v.root.querySelector(".composer")).toBeNull());
    open();
    flush(() => [...v.root.querySelectorAll("button")].find(b => b.textContent === "Cancel")!.click());
    expect(v.root.querySelector(".composer")).toBeNull();
    expect(onPost).toHaveBeenCalledTimes(1);
    v.unmount();
  });

  it("keeps the composer and its text when the post fails", async () => {
    const onPost = vi.fn(async () => { throw new Error("offline"); });
    const v = mount(NewComment, { version: 1, onPost });
    flush(() => v.root.querySelector<HTMLButtonElement>(".new-comment-open")!.click());
    const ta = v.root.querySelector("textarea")!;
    flush(() => { ta.value = "Warmer"; ta.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => v.root.querySelector<HTMLButtonElement>("button[type=submit]")!.click());
    await vi.waitFor(() => expect(onPost).toHaveBeenCalled());
    await Promise.resolve();
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
    const v = mount(Sidebar, { ...props, compose: { version: 1, onPost: vi.fn(async () => {}) } });
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
});
