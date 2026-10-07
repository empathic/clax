import { afterEach, describe, expect, it, vi } from "vitest";
import PeoplePanel from "./ui/PeoplePanel.svelte";
import { flush, mount } from "./test/svelte";

const resolvedThread = { id: "t9", status: "resolved", comments: [] };
function view(askName: string | null, status = "resolved") {
  return {
    data: { artifact: { participants: { people: [], agents: [] } }, versions: [] }, agents: [], presence: [], working: [], attention: null,
    threads: [{ ...resolvedThread, status }], me: { public_id: "u_1", display_name: null, created_at: "x" }, shareWhere: false, commenting: false, askName,
  } as never;
}

describe("PeoplePanel asking for a name to reopen", () => {
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

  it("asks in the menu, not as a failure, with focus in the name field; closed with no name, it forgets the reopen and focus returns to the thread's card", () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: null, created_at: "x" } }))));
    const card = document.createElement("article");
    card.className = "thread-card";
    card.dataset.thread = "t9";
    card.innerHTML = `<button class="card-head">«Our team»</button>`;
    document.body.append(card);
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const onClose = vi.fn();
    const v = mount(PeoplePanel, { ctl: ctl as never, s: view("t9"), onClose });
    expect(v.root.querySelector(".ask")!.textContent).toBe("Add your name to reopen the thread: people see it beside what you do.");
    expect(v.root.querySelector(".ask")!.getAttribute("role")).toBe("status");
    expect(document.activeElement).toBe(v.root.querySelector("input.viewer-name"));
    v.root.querySelector<HTMLElement>(".people")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(ctl.forgetReopen).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(card.querySelector(".card-head"));
    v.unmount();
  });

  it("closes itself once the thread is open again, and opened from the roster asks nothing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: "Mia", created_at: "x" } }))));
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const onClose = vi.fn();
    const v = mount(PeoplePanel, { ctl: ctl as never, s: view("t9"), onClose });
    expect(onClose).not.toHaveBeenCalled();
    v.update({ ctl: ctl as never, s: view("t9", "open"), onClose });
    flush();
    expect(onClose).toHaveBeenCalledOnce();
    expect(ctl.forgetReopen).not.toHaveBeenCalled();
    v.unmount();
    const plain = mount(PeoplePanel, { ctl: ctl as never, s: view(null), onClose });
    expect(plain.root.querySelector(".ask")).toBeNull();
    expect(document.activeElement).toBe(plain.root.querySelector(".people"));
    plain.unmount();
  });
});
