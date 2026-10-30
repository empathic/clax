import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import RemoteMutes from "../bot/RemoteMutes.svelte";
import { item } from "./fixtures";
import RemoteRequestCard from "./RemoteRequestCard.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const button = (root: Element, text: string) => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent!.trim() === text) ?? null;
const now = new Date("2026-10-07T09:17:03.120Z");
const ID = "01J9Q0000000000000000000RR";
const HOSTILE = `<img src=x onerror="window.pwned=1">`;

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); document.body.replaceChildren(); });

describe("RemoteRequestCard", () => {
  it("says which agent wants to connect which instance to read what, why, with the three answers", () => {
    const m = mount(RemoteRequestCard, { item: item("remote_request"), now });
    const card = m.root.querySelector(".rcard")!;
    expect(card.getAttribute("data-request")).toBe(ID);
    expect(card.querySelector(".chead > span")!.textContent).toBe("claude wants to connect to Acme Clax (https://clax.example) to read https://clax.example/a/7q3k9mzx2b4t");
    expect(card.querySelector(".reason")!.textContent).toBe("“to read the launch plan”");
    expect([...card.querySelectorAll(".acts button")].map(b => b.textContent!.trim())).toEqual(["Connect", "Not now", "Never for this instance"]);
  });

  it("Connect starts the sign-in and opens the instance's approval page in a new tab", async () => {
    const calls = stubFetch({ [`POST /api/remotes/requests/${ID}/answer`]: () => ({ request: { status: "open" }, verification_url: "https://clax.example/device?code=ABCD-1234", user_code: "ABCD-1234" }) });
    const tab = { opener: {}, location: { href: "" }, close: vi.fn() };
    const open = vi.spyOn(window, "open").mockReturnValue(tab as unknown as Window);
    const m = mount(RemoteRequestCard, { item: item("remote_request"), now });
    click(button(m.root, "Connect")!);
    await settle();
    expect(open).toHaveBeenCalledWith("", "_blank");
    expect(calls).toEqual([{ url: `/api/remotes/requests/${ID}/answer`, method: "POST", body: { answer: "connect" } }]);
    expect(tab.location.href).toBe("https://clax.example/device?code=ABCD-1234");
    expect(tab.opener).toBeNull();
    expect(m.root.querySelector(".outcome")!.textContent).toContain("Approve on Acme Clax in the new tab");
    expect(m.root.querySelector(".acts")).toBeNull();
  });

  it("links the approval page when the browser kept the tab from opening", async () => {
    stubFetch({ [`POST /api/remotes/requests/${ID}/answer`]: () => ({ request: { status: "open" }, verification_url: "https://clax.example/device?code=ABCD-1234" }) });
    vi.spyOn(window, "open").mockReturnValue(null);
    const m = mount(RemoteRequestCard, { item: item("remote_request"), now });
    click(button(m.root, "Connect")!);
    await settle();
    expect(m.root.querySelector<HTMLAnchorElement>(".rcard a[target=_blank]")!.href).toBe("https://clax.example/device?code=ABCD-1234");
  });

  it("Not now and Never answer, and say how it went", async () => {
    for (const [label, answer, status, says] of [["Not now", "not_now", "dismissed", "Not connected"], ["Never for this instance", "never", "muted", "unmute it in Settings"]] as const) {
      const calls = stubFetch({ [`POST /api/remotes/requests/${ID}/answer`]: () => ({ request: { status } }) });
      const open = vi.spyOn(window, "open");
      const m = mount(RemoteRequestCard, { item: item("remote_request"), now });
      click(button(m.root, label)!);
      await settle();
      expect(calls.map(c => c.body)).toEqual([{ answer }]);
      expect(open).not.toHaveBeenCalled();
      expect(m.root.querySelector(".rcard")!.getAttribute("data-status")).toBe(status);
      expect(m.root.querySelector(".outcome")!.textContent).toContain(says);
      document.body.replaceChildren();
    }
  });

  it("shows the daemon's refusal and keeps the answers", async () => {
    stubFetch({ [`POST /api/remotes/requests/${ID}/answer`]: [403, { error: { code: "browser_required", message: "answering a connection request needs the owner's browser" } }] });
    const m = mount(RemoteRequestCard, { item: item("remote_request"), now });
    click(button(m.root, "Not now")!);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("Answering a connection request needs the owner's browser.");
    expect(button(m.root, "Connect")).not.toBeNull();
  });

  it("an answered or expired request says so, without the answers", () => {
    const base = item("remote_request").remote_request!;
    for (const [status, says] of [["connected", "Connected"], ["expired", "Expired"]] as const) {
      const m = mount(RemoteRequestCard, { item: item("remote_request", { remote_request: { ...base, status } }), now });
      expect(m.root.querySelector(".outcome")!.textContent).toContain(says);
      expect(m.root.querySelector(".acts")).toBeNull();
      document.body.replaceChildren();
    }
  });

  it("shows names, reasons and URLs as text", () => {
    const base = item("remote_request").remote_request!;
    const m = mount(RemoteRequestCard, { item: item("remote_request", { remote_request: { ...base, name: HOSTILE, reason: HOSTILE, url: HOSTILE } }), now });
    expect(m.root.querySelector("img")).toBeNull();
    expect(m.root.querySelector(".chead")!.textContent).toContain(HOSTILE);
  });
});

describe("Settings, Remotes", () => {
  it("lists the muted instances and unmutes one", async () => {
    const calls = stubFetch({
      "GET /api/remotes/mutes": { mutes: [{ origin: "https://clax.example", muted_at: "2026-10-07T09:00:00.000Z" }, { origin: "https://b.example", muted_at: "2026-10-06T09:00:00.000Z" }] },
      [`DELETE /api/remotes/mutes/${encodeURIComponent("https://clax.example")}`]: { origin: "https://clax.example", unmuted: true },
    });
    const m = mount(RemoteMutes, {});
    await settle();
    expect([...m.root.querySelectorAll(".rmutes .origin")].map(e => e.textContent)).toEqual(["https://clax.example", "https://b.example"]);
    click(m.root.querySelectorAll<HTMLButtonElement>(".rmutes li button")[0]);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/remotes/mutes/https%3A%2F%2Fclax.example", method: "DELETE", body: undefined });
    expect([...m.root.querySelectorAll(".rmutes .origin")].map(e => e.textContent)).toEqual(["https://b.example"]);
  });
});
