import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { item } from "../q/fixtures";
import { flush, mount } from "../test/svelte";
import ConnectionCard from "./ConnectionCard.svelte";
import { grant, settle, stubFetch } from "./fixtures";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const type = (el: HTMLInputElement, v: string) => flush(() => { el.value = v; el.dispatchEvent(new Event("input", { bubbles: true })); });
const button = (root: Element, text: string) => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent!.trim() === text) ?? null;
const now = new Date("2026-10-07T09:17:03.120Z");
const HOSTILE = `<img src=x onerror="window.pwned=1">`;

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("ConnectionCard", () => {
  it("shows the request's machine, address and key", () => {
    const m = mount(ConnectionCard, { item: item("connection"), now });
    const card = m.root.querySelector(".ccard")!;
    expect(card.getAttribute("data-grant")).toBe("g_abc");
    expect(card.querySelector(".chead")!.textContent).toContain("Connection request from sam-mbp (192.168.1.40)");
    expect(card.textContent).toContain("key SHA256:9f2c");
  });

  it("enables Approve only with a typed code and a name for the new person, and approves with both and the preset", async () => {
    const calls = stubFetch({ "POST /api/grants/g_abc/approve": () => ({ grant: grant({ id: "g_abc", status: "approved", via: "request", person: { public_id: "p7", display_name: "Sam" } }) }) });
    const m = mount(ConnectionCard, { item: item("connection"), now });
    expect(m.root.querySelector("form")).toBeNull();
    click(button(m.root, "Approve…")!);
    await settle();
    const code = m.root.querySelector<HTMLInputElement>('input[name="code"]')!;
    const name = m.root.querySelector<HTMLInputElement>('input[name="name"]')!;
    expect(document.activeElement).toBe(code);
    const approve = button(m.root, "Approve")!;
    expect(approve.disabled).toBe(true);
    type(code, "wxyz-4821");
    expect(approve.disabled).toBe(true);
    type(name, "   ");
    expect(approve.disabled).toBe(true);
    type(name, "Sam");
    expect(approve.disabled).toBe(false);
    const preset = m.root.querySelector<HTMLSelectElement>("select")!;
    expect(preset.value).toBe("clax:collaborator");
    preset.value = "clax:reader";
    flush(() => { preset.dispatchEvent(new Event("change", { bubbles: true })); });
    click(approve);
    await settle();
    expect(calls).toEqual([{ url: "/api/grants/g_abc/approve", method: "POST", body: { user_code: "wxyz-4821", name: "Sam", rights: "clax:reader" } }]);
    expect(m.root.querySelector(".outcome")!.textContent).toContain("Approved: Sam is connected");
    expect(m.root.querySelector("form")).toBeNull();
  });

  it("links to Invite for an existing person or the owner's own machine", async () => {
    const m = mount(ConnectionCard, { item: item("connection"), now });
    click(button(m.root, "Approve…")!);
    await settle();
    const invite = [...m.root.querySelectorAll("a")].find(a => a.textContent === "Invite")!;
    expect(invite.getAttribute("href")).toBe("/people?invite");
    expect(invite.closest("p")!.textContent).toContain("someone already connected, or to you");
  });

  it("shows a wrong code's refusal and keeps the form", async () => {
    stubFetch({ "POST /api/grants/g_abc/approve": [400, { error: { code: "wrong_code", message: "That is not the code the machine shows" } }] });
    const m = mount(ConnectionCard, { item: item("connection"), now });
    click(button(m.root, "Approve…")!);
    await settle();
    type(m.root.querySelector<HTMLInputElement>('input[name="code"]')!, "AAAA-0000");
    type(m.root.querySelector<HTMLInputElement>('input[name="name"]')!, "Sam");
    click(button(m.root, "Approve")!);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("That is not the code the machine shows");
    expect(m.root.querySelector("form")).not.toBeNull();
  });

  it("denies, and says a decided request's outcome", async () => {
    const calls = stubFetch({ "POST /api/grants/g_abc/deny": () => ({ grant: grant({ id: "g_abc", status: "denied", via: "request", person: null }) }) });
    const m = mount(ConnectionCard, { item: item("connection"), now });
    click(button(m.root, "Deny")!);
    await settle();
    expect(calls.map(c => `${c.method} ${c.url}`)).toEqual(["POST /api/grants/g_abc/deny"]);
    expect(m.root.querySelector(".outcome")!.textContent).toContain("Denied");
    expect(button(m.root, "Approve…")).toBeNull();
  });

  it("closes the form with Escape, focus back on Approve…, and holds Approve… while a Deny is in flight", async () => {
    let answer: (r: Response) => void = () => {};
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(r => { answer = r; })));
    const m = mount(ConnectionCard, { item: item("connection"), now });
    click(button(m.root, "Approve…")!);
    await settle();
    const code = m.root.querySelector<HTMLInputElement>('input[name="code"]')!;
    flush(() => { code.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
    await settle();
    expect(m.root.querySelector("form")).toBeNull();
    expect(document.activeElement).toBe(button(m.root, "Approve…"));
    click(button(m.root, "Deny")!);
    expect(button(m.root, "Approve…")!.disabled).toBe(true);
    answer(new Response(JSON.stringify({ grant: grant({ id: "g_abc", status: "denied", via: "request", person: null }) })));
    await settle();
    expect(m.root.querySelector(".outcome")!.textContent).toContain("Denied");
  });

  it("offers Mark read on an unread request that expired, and on no pending one", () => {
    const expired = item("connection", { read: false, connection: { ...item("connection").connection!, status: "expired" } });
    const onRead = vi.fn();
    const m = mount(ConnectionCard, { item: expired, now, onRead });
    expect(m.root.querySelector(".outcome")!.textContent).toBe("Expired: the machine stopped waiting");
    click(button(m.root, "Mark read")!);
    expect(onRead).toHaveBeenCalledWith(expired);
    m.unmount();
    const pending = mount(ConnectionCard, { item: item("connection", { read: false }), now, onRead });
    expect(button(pending.root, "Mark read")).toBeNull();
    pending.unmount();
    const read = mount(ConnectionCard, { item: { ...expired, read: true }, now, onRead });
    expect(button(read.root, "Mark read")).toBeNull();
  });

  it("keeps the machine's name as text", () => {
    const req = item("connection", { connection: { id: "g", status: "pending", via: "request", client: { name: `${HOSTILE}box`, key: null, peer_ip: "10.0.0.2" }, person: null } });
    const m = mount(ConnectionCard, { item: req, now });
    expect(m.root.querySelector("img")).toBeNull();
    expect(m.root.querySelector(".chead")!.textContent).toContain(`${HOSTILE}box`);
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
  });
});
