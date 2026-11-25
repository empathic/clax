import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import { queueRequest } from "./fixtures";
import QueueRequestCard from "./QueueRequestCard.svelte";

/** Passkeys not required: a held decision is confirmed on its card. */
const NO_PASSKEY = { available: true, require_passkey: false, required: false, passkeys: [] };

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const button = (root: Element, text: string) => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent!.trim() === text) ?? null;
const now = new Date("2026-10-07T09:17:03.120Z");
const ID = "01J9Q0000000000000000000QR";
const HOSTILE = `<img src=x onerror="window.pwned=1">`;
const QUEUE = { id: "q9", name: "Site feedback", origins: ["https://app.example"], reporters_see: "own", created_at: "2026-10-07T09:18:00Z", role: "admin" };
const approved = queueRequest({ status: "approved", decided_at: "2026-10-07T09:18:00Z", queue_id: "q9" });

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); document.body.replaceChildren(); });

describe("QueueRequestCard", () => {
  it("says which agent asks for which queue, its origins, what reporters see, its note and expiry, with Approve and Deny", () => {
    const m = mount(QueueRequestCard, { request: queueRequest(), now });
    const card = m.root.querySelector(".qrcard")!;
    expect(card.getAttribute("data-queue-request")).toBe(ID);
    expect(card.getAttribute("data-status")).toBe("pending");
    expect(card.querySelector(".chead > span")!.textContent).toBe("Claude Code asks for a report queue “Site feedback”");
    expect([...card.querySelectorAll(".origins li")].map(l => l.textContent)).toEqual(["https://app.example", "https://staging.app.example"]);
    expect(card.querySelector(".sees")!.textContent).toBe("Reporters see only their own reports");
    expect(card.querySelector(".note")!.textContent).toBe("“for the beta testers' bug reports”");
    expect(card.querySelector(".expires")!.textContent).toBe("Expires in 55 min");
    expect([...card.querySelectorAll(".acts button")].map(b => b.textContent!.trim())).toEqual(["Approve", "Deny"]);
    expect(card.querySelector(".acts .primary")!.textContent).toBe("Approve");
    expect(card.querySelector(".what")!.textContent).toBe("Approve makes the queue with you as its admin and gives the agent one service grant for it, once.");
  });

  it("says when reporters see every report", () => {
    const m = mount(QueueRequestCard, { request: queueRequest({ reporters_see: "all", note: null }), now });
    expect(m.root.querySelector(".sees")!.textContent).toBe("Reporters see every report in the queue");
    expect(m.root.querySelector(".note")).toBeNull();
  });

  it("Approve posts to approve and says the queue is made, linking it", async () => {
    const calls = stubFetch({ [`POST /api/queue-requests/${ID}/approve`]: { request: approved, queue: QUEUE } });
    const onDecided = vi.fn();
    const m = mount(QueueRequestCard, { request: queueRequest(), now, onDecided });
    click(button(m.root, "Approve")!);
    await settle();
    expect(calls).toEqual([{ url: `/api/queue-requests/${ID}/approve`, method: "POST", body: {} }]);
    expect(m.root.querySelector(".qrcard")!.getAttribute("data-status")).toBe("approved");
    expect(m.root.querySelector(".outcome")!.textContent).toBe("Approved: the queue is made. Open Site feedback");
    expect(m.root.querySelector<HTMLAnchorElement>(".outcome a")!.getAttribute("href")).toBe("/queues/q9");
    expect(m.root.querySelector(".acts")).toBeNull();
    expect(onDecided).toHaveBeenCalledWith(approved);
  });

  it("Deny posts to deny and says so", async () => {
    const calls = stubFetch({ [`POST /api/queue-requests/${ID}/deny`]: { request: queueRequest({ status: "denied", decided_at: "2026-10-07T09:18:00Z" }) } });
    const m = mount(QueueRequestCard, { request: queueRequest(), now });
    click(button(m.root, "Deny")!);
    await settle();
    expect(calls).toEqual([{ url: `/api/queue-requests/${ID}/deny`, method: "POST", body: {} }]);
    expect(m.root.querySelector(".outcome")!.textContent).toBe("Denied");
    expect(m.root.querySelector(".acts")).toBeNull();
  });

  it("a decision on a request no longer pending shows how it stands", async () => {
    stubFetch({ [`POST /api/queue-requests/${ID}/approve`]: [409, { error: { code: "request_closed", message: "the request is no longer pending", request: queueRequest({ status: "cancelled" }) } }] });
    const m = mount(QueueRequestCard, { request: queueRequest(), now });
    click(button(m.root, "Approve")!);
    await settle();
    expect(m.root.querySelector(".qrcard")!.getAttribute("data-status")).toBe("cancelled");
    expect(m.root.querySelector(".outcome")!.textContent).toBe("Cancelled: the agent's access ended before you answered");
    expect(m.root.querySelector('[role="alert"]')).toBeNull();
  });

  it("shows the daemon's refusal and keeps the buttons", async () => {
    stubFetch({ [`POST /api/queue-requests/${ID}/deny`]: [403, { error: { code: "forbidden", message: "only the request's approver decides it" } }] });
    const m = mount(QueueRequestCard, { request: queueRequest(), now });
    click(button(m.root, "Deny")!);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Only the request's approver decides it.");
    expect(button(m.root, "Approve")).not.toBeNull();
  });

  it("a held decision says where to confirm, and deciding again presents the confirmation", async () => {
    const URL = "http://localhost:7480/inbox?confirm=cf_01";
    const sent: { url: string; body: unknown; confirm: string | null }[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/api/passkeys") return new Response(JSON.stringify(NO_PASSKEY));
      const confirm = new Headers(init?.headers).get("x-clax-confirmation");
      sent.push({ url, body: JSON.parse(String(init?.body)), confirm });
      return confirm === "cf_01"
        ? new Response(JSON.stringify({ request: approved, queue: QUEUE }))
        : new Response(JSON.stringify({ status: "confirmation_pending", confirmation_id: "cf_01", confirm_url: URL }), { status: 202 });
    }));
    const m = mount(QueueRequestCard, { request: queueRequest(), now });
    click(button(m.root, "Approve")!);
    // A held decision first asks whether a passkey confirms it here.
    await vi.waitFor(() => { flush(); expect(m.root.querySelector(".held")?.textContent).toContain(`Confirm in your browser: ${URL}`); });
    expect(m.root.querySelector('[role="alert"]')).toBeNull();
    expect(m.root.querySelector(".qrcard")!.getAttribute("data-status")).toBe("pending");
    click(button(m.root, "Approve")!);
    await settle();
    const path = `/api/queue-requests/${ID}/approve`;
    expect(sent).toEqual([{ url: path, body: {}, confirm: null }, { url: path, body: {}, confirm: "cf_01" }]);
    expect(m.root.querySelector(".held")).toBeNull();
    expect(m.root.querySelector(".outcome")!.textContent).toContain("Approved: the queue is made");
  });

  it("a decided, cancelled or expired request says how it went, without the buttons", () => {
    for (const [status, says] of [
      ["denied", "Denied"],
      ["cancelled", "Cancelled: the agent's access ended before you answered"],
      ["expired", "Expired: nobody answered in time"],
    ] as const) {
      const m = mount(QueueRequestCard, { request: queueRequest({ status }), now });
      expect(m.root.querySelector(".outcome")!.textContent).toBe(says);
      expect(m.root.querySelector(".acts")).toBeNull();
      document.body.replaceChildren();
    }
    const m = mount(QueueRequestCard, { request: approved, now });
    expect(m.root.querySelector(".outcome")!.textContent).toBe("Approved: the queue is made. Open Site feedback");
  });

  it("says a request that is gone is gone", () => {
    const m = mount(QueueRequestCard, { request: null, now });
    expect(m.root.querySelector(".outcome")!.textContent).toBe("This request is gone");
    expect(m.root.querySelector(".acts")).toBeNull();
  });

  it("shows names, notes, agents and origins as text", () => {
    const m = mount(QueueRequestCard, { request: queueRequest({ name: HOSTILE, note: HOSTILE, agent: HOSTILE, origins: [HOSTILE] }), now });
    expect(m.root.querySelector("img")).toBeNull();
    expect(m.root.querySelector(".chead")!.textContent).toContain(HOSTILE);
    expect(m.root.querySelector(".note")!.textContent).toContain(HOSTILE);
    expect(m.root.querySelector(".origins li")!.textContent).toBe(HOSTILE);
  });
});
