// The artifact's tag editor (ui/TagEditor.svelte) and its popovers
// (ui/tag-editor.ts): changes sent one after another, none lost; a refusal
// shown with its box unchecked; answers applied in the order asked; and the
// popover closing on a press outside, focus leaving, Escape, and the view
// ending.
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TagRef } from "./api";
import { flush } from "./test/svelte";
import { ArtifactController } from "./view/artifact-controller";
import { Store } from "./view/store";

const ID = "7q3k9mzx2b4t";
const TAGS: TagRef[] = [{ id: "t_aaaaaaaaaa", name: "design", colour: "purple" }, { id: "t_bbbbbbbbbb", name: "q4", colour: "blue" }];
const LIST = { tags: TAGS.map(t => ({ ...t, description: null, created_at: "", updated_at: "", count: 0 })), colours: ["gray"], may_manage: true };

/** A view's controller, as far as the editor reads and writes it. */
function fakeCtl() {
  let asked = 0;
  let shown = 0;
  const ctl = {
    id: ID, disposed: false,
    state: new Store<Record<string, unknown>>({ tags: null, data: { artifact: { tags: [], may_tag: true } } }),
    tagTicket: () => ++asked,
    setTags(t: number, v: unknown) { if (t < shown) return; shown = t; ctl.state.set({ tags: v }); },
  };
  return ctl;
}
const settle = async () => { for (let i = 0; i < 10; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
const boxes = () => [...document.querySelectorAll<HTMLInputElement>(".tag-pop input[type=checkbox]")];

/** `fetch` answering the tag list, and each PATCH when the test says (`answer`). */
function stub() {
  const patches: { body: { add: string[]; remove: string[] }; answer(status: number, body: unknown): void }[] = [];
  vi.stubGlobal("fetch", vi.fn((url: string, init?: RequestInit) => {
    if (url === "/api/token") return Promise.resolve(new Response("{}", { status: 403 }));
    if (url === "/api/tags") return Promise.resolve(new Response(JSON.stringify(LIST)));
    if (init?.method === "PATCH") return new Promise(resolve => patches.push({ body: JSON.parse(String(init.body)), answer: (status, body) => resolve(new Response(JSON.stringify(body), { status })) }));
    return Promise.resolve(new Response("{}", { status: 404 }));
  }));
  return patches;
}

async function openEditor(ctl = fakeCtl()) {
  const button = document.createElement("button");
  document.body.append(button);
  const { toggleTagEditor } = await import("./ui/tag-editor");
  flush(() => toggleTagEditor(button, ctl as unknown as ArtifactController));
  await settle();
  return { ctl, button, toggleTagEditor };
}

afterEach(() => { if (document.querySelector(".tag-pop")) document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })); vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("the tag editor", () => {
  it("sends each change in turn, losing none made while one is on its way", async () => {
    const patches = stub();
    const { ctl } = await openEditor();
    flush(() => boxes()[0].click());
    flush(() => boxes()[1].click());
    await settle();
    // Both show checked at once; one request at a time.
    expect(boxes().map(b => b.checked)).toEqual([true, true]);
    expect(patches).toHaveLength(1);
    expect(patches[0].body).toEqual({ add: [TAGS[0].id], remove: [] });
    patches[0].answer(200, { artifact_id: ID, tags: [TAGS[0]], changed: true });
    await settle();
    expect(patches).toHaveLength(2);
    expect(patches[1].body).toEqual({ add: [TAGS[1].id], remove: [] });
    patches[1].answer(200, { artifact_id: ID, tags: TAGS, changed: true });
    await settle();
    expect((ctl.state.get().tags as { tags: TagRef[] }).tags).toEqual(TAGS);
    expect(boxes().map(b => b.checked)).toEqual([true, true]);
  });

  it("shows a refusal and unchecks its box", async () => {
    const patches = stub();
    await openEditor();
    flush(() => boxes()[1].click());
    await settle();
    patches[0].answer(400, { error: { code: "too_many_tags", message: "an artifact carries at most 20 tags" } });
    await settle();
    expect(document.querySelector(".tag-pop [role=alert]")!.textContent).toBe("an artifact carries at most 20 tags");
    expect(boxes()[1].checked).toBe(false);
  });

  it("closes on a press outside, on focus leaving, on Escape (focus back on its button) and when the view ends", async () => {
    stub();
    const { ctl, button, toggleTagEditor } = await openEditor();
    const pop = () => document.querySelector(".tag-pop");
    expect(button.getAttribute("aria-expanded")).toBe("true");
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(pop()).toBeNull();
    expect(button.getAttribute("aria-expanded")).toBe("false");
    const reopen = async () => { flush(() => toggleTagEditor(button, ctl as unknown as ArtifactController)); await settle(); expect(pop()).not.toBeNull(); };
    await reopen();
    const elsewhere = document.createElement("input");
    document.body.append(elsewhere);
    pop()!.querySelector<HTMLInputElement>("input[type=search]")!.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: elsewhere }));
    expect(pop()).toBeNull();
    await reopen();
    flush(() => pop()!.querySelector("input[type=search]")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(pop()).toBeNull();
    expect(document.activeElement).toBe(button);
    await reopen();
    ctl.disposed = true;
    ctl.state.set({ tags: { tags: [], may: true } });
    await settle();
    expect(pop()).toBeNull();
    ctl.disposed = false;
    await reopen();
    // The button goes (the viewer may no longer tag).
    button.remove();
    ctl.state.set({ tags: { tags: [], may: false } });
    await settle();
    expect(pop()).toBeNull();
  });
});

describe("the view's tags", () => {
  it("never shows an answer older than one shown", () => {
    const ctl = new ArtifactController({ id: ID, pinnedVersion: null });
    const first = ctl.tagTicket();
    const second = ctl.tagTicket();
    ctl.setTags(second, { tags: TAGS, may: true });
    ctl.setTags(first, { tags: [TAGS[0]], may: true });
    expect(ctl.state.get().tags).toEqual({ tags: TAGS, may: true });
    ctl.setTags(ctl.tagTicket(), { tags: [], may: true });
    expect(ctl.state.get().tags).toEqual({ tags: [], may: true });
    ctl.dispose();
  });
});
