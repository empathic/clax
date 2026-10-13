import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "./fixtures";
import InviteDialog from "./InviteDialog.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const type = (el: HTMLInputElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });
const submit = (root: Element) => flush(() => { root.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
const now = new Date("2026-10-09T10:00:00Z");
const made = { invite: { id: "i1", command: "clax connect http://192.168.1.5:7480/i/K7QF-M2XD-9PLA", expires_at: "2026-10-10T10:00:00Z", person: { public_id: "p1", display_name: "Sam" } } };

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("InviteDialog", () => {
  it("invites a new person at Collaborator, then shows the command, its expiry, the LAN warning and, on the first invite, the switch", async () => {
    const calls = stubFetch({ "POST /api/people/invites": made, "POST /api/approvals": { approval: { id: "ap2" } } });
    const onMade = vi.fn();
    const m = mount(InviteDialog, { first: true, unconnected: { on: false, approval_id: null }, now, onMade, onClose: vi.fn(), onSwitched: vi.fn() });
    const preset = m.root.querySelector<HTMLSelectElement>("select")!;
    expect(preset.value).toBe("clax:collaborator");
    type(m.root.querySelector<HTMLInputElement>('input[name="name"]')!, "Sam");
    submit(m.root);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/people/invites", method: "POST", body: { name: "Sam", rights: "clax:collaborator" } });
    expect(m.root.querySelector("code")!.textContent).toBe(made.invite.command);
    const text = m.root.textContent!;
    expect(text).toContain("Expires in 24 h, one use");
    expect(text).toContain("Use a trusted LAN: Clax traffic is not encrypted");
    expect(m.root.querySelector('[role="switch"]')).not.toBeNull();
    expect(onMade).toHaveBeenCalled();
    // Turning the switch on from here makes the owner's approval of `*unconnected`.
    click(m.root.querySelector('[role="switch"]')!);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/approvals", method: "POST", body: { author: "*unconnected", scope: "everywhere" } });
  });

  it("shows no switch beside a later invite, and invites the owner's own machine without rights", async () => {
    const calls = stubFetch({ "POST /api/people/invites": made });
    const m = mount(InviteDialog, { first: false, unconnected: { on: true, approval_id: "ap1" }, now, onMade: vi.fn(), onClose: vi.fn(), onSwitched: vi.fn() });
    click(m.root.querySelector<HTMLInputElement>('input[value="me"]')!);
    expect(m.root.querySelector('input[name="name"]')).toBeNull();
    submit(m.root);
    await settle();
    expect(calls.at(-1)!.body).toEqual({ me: true });
    expect(m.root.querySelector("code")).not.toBeNull();
    expect(m.root.querySelector('[role="switch"]')).toBeNull();
  });

  it("says what the daemon says when it listens on this machine only, and makes no invite", async () => {
    const msg = "Clax listens on this machine only: restart it with `clax serve --bind 0.0.0.0` to invite people";
    stubFetch({ "POST /api/people/invites": [409, { error: { code: "not_on_lan", message: msg } }] });
    const m = mount(InviteDialog, { first: true, unconnected: { on: true, approval_id: "ap1" }, now, onMade: vi.fn(), onClose: vi.fn(), onSwitched: vi.fn() });
    type(m.root.querySelector<HTMLInputElement>('input[name="name"]')!, "Sam");
    submit(m.root);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe(msg);
    expect(m.root.querySelector("code")).toBeNull();
  });
});
