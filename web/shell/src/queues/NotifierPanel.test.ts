import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import NotifierPanel from "./NotifierPanel.svelte";

const QUEUE = { id: "q1", name: "Bugs", origins: [], reporters_see: "own" as const, public_notice: false, created_at: "2026-10-27T10:00:00Z", role: "admin" as const };
const KEY = { set: true, last4: "wxyz", set_at: "2026-11-05T10:00:00Z", set_by: { id: "*owner", name: "Ana" }, last_ok_at: null, host: "https://acme.zulipchat.com", problem: null };
const ZULIP = {
  kind: "zulip", rev: "r1", site: "https://acme.zulipchat.com", bot_email: "clax-bot@acme.zulipchat.com", stream: "bugs", people: { "ana-dev": "ana@acme.dev" },
  api_key: KEY, last_ok_at: null, last_failure: null, last_failure_at: null, pending: 0, expired: 0, set_by: { id: "*owner", name: "Ana" }, set_at: "2026-11-05T10:00:00Z",
};
const ROUTE = "/api/queues/q1/notifiers/zulip";
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const type = (el: Element | null, v: string) => flush(() => { const i = el as HTMLInputElement | HTMLTextAreaElement; i.value = v; i.dispatchEvent(new Event("input", { bubbles: true })); });
const button = (root: ParentNode, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent!.trim().startsWith(text));
const open = async (routes: Record<string, unknown>) => {
  const calls = stubFetch(routes);
  const m = mount(NotifierPanel, { queue: QUEUE });
  await settle();
  return { calls, m };
};

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("NotifierPanel", () => {
  it("connects Zulip with the key, and reads people as login and address", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: { notifier: null }, [`PUT ${ROUTE}`]: { notifier: ZULIP } });
    expect(m.root.textContent).toContain("Not connected.");
    type(m.root.querySelector('input[name="site"]'), " https://acme.zulipchat.com ");
    type(m.root.querySelector('input[name="bot_email"]'), "clax-bot@acme.zulipchat.com");
    type(m.root.querySelector('input[name="api_key"]'), "the-key-0000");
    type(m.root.querySelector('input[name="stream"]'), "bugs");
    type(m.root.querySelector('textarea[name="people"]'), "Ana-Dev ana@acme.dev\n\n");
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(calls.find(c => c.method === "PUT")).toEqual({
      url: ROUTE, method: "PUT",
      body: { site: "https://acme.zulipchat.com", bot_email: "clax-bot@acme.zulipchat.com", api_key: "the-key-0000", stream: "bugs", people: { "Ana-Dev": "ana@acme.dev" }, if_rev: null },
    });
    expect((m.root.querySelector('input[name="api_key"]') as HTMLInputElement).value).toBe("");
  });

  it("shows only the key's last four, and a save without one keeps it", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: { notifier: ZULIP }, [`PUT ${ROUTE}`]: { notifier: ZULIP } });
    expect(m.root.textContent).toContain("wxyz");
    expect((m.root.querySelector('textarea[name="people"]') as HTMLTextAreaElement).value).toBe("ana-dev ana@acme.dev");
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    const body = calls.find(c => c.method === "PUT")!.body as Record<string, unknown>;
    expect(body).not.toHaveProperty("api_key");
    expect(body.if_rev).toBe("r1");
  });

  it("says a line of people it cannot read, and sends nothing", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: { notifier: ZULIP } });
    type(m.root.querySelector('textarea[name="people"]'), "just-a-login");
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(m.root.querySelector(".err")!.textContent).toContain("just-a-login");
    expect(calls.some(c => c.method === "PUT")).toBe(false);
  });

  it("shows a failure in the daemon's fixed words, with what waits", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: { notifier: { ...ZULIP, last_failure: "Zulip refused the bot's email or API key.", pending: 2, expired: 1 } } });
    const stat = m.root.querySelector(".stat")!.textContent!;
    expect(stat).toContain("Zulip refused the bot's email or API key.");
    expect(m.root.textContent).toContain("2 notices waiting");
    expect(m.root.textContent).toContain("1 notice could not be sent");
  });

  it("sends a test message, and says how it went", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: { notifier: ZULIP }, [`POST ${ROUTE}/test`]: { notifier: { ...ZULIP, last_ok_at: "2026-11-06T10:00:00Z" } } });
    click(button(m.root, "Send a test message"));
    await settle();
    expect(calls.find(c => c.method === "POST")).toEqual({ url: `${ROUTE}/test`, method: "POST", body: { if_rev: "r1" } });
    expect(m.root.querySelector("[data-tested]")!.textContent).toContain("Sent");
  });

  it("shows why a test message failed", async () => {
    const { m } = await open({
      [`GET ${ROUTE}`]: { notifier: ZULIP },
      [`POST ${ROUTE}/test`]: [502, { error: { code: "notifier_failed", message: "Zulip could not be reached." } }],
    });
    click(button(m.root, "Send a test message"));
    await settle();
    expect(m.root.querySelector(".err")!.textContent).toContain("Zulip could not be reached.");
  });

  it("reads the notifier again when another admin changed it", async () => {
    const { calls, m } = await open({
      [`GET ${ROUTE}`]: { notifier: ZULIP },
      [`PUT ${ROUTE}`]: [400, { error: { code: "notifier_changed", message: "this notifier has changed since it was read: read it again" } }],
    });
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(calls.filter(c => c.method === "GET" && c.url === ROUTE).length).toBe(2);
    expect(m.root.querySelector(".err")!.textContent).toContain("changed");
  });

  it("disconnects by the rev it read, once a person confirms", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: { notifier: ZULIP }, [`DELETE ${ROUTE}`]: {} });
    vi.stubGlobal("confirm", () => false);
    click(button(m.root, "Disconnect"));
    await settle();
    expect(calls.some(c => c.method === "DELETE")).toBe(false);
    vi.stubGlobal("confirm", () => true);
    click(button(m.root, "Disconnect"));
    await settle();
    expect(calls.find(c => c.method === "DELETE")!.body).toEqual({ if_rev: "r1" });
    expect(m.root.textContent).toContain("Not connected.");
  });

  it("keeps what was typed and not saved after a test, which used the saved settings", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: { notifier: ZULIP }, [`POST ${ROUTE}/test`]: { notifier: { ...ZULIP, last_ok_at: "2026-11-06T10:00:00Z" } } });
    type(m.root.querySelector('input[name="stream"]'), "typed-not-saved");
    type(m.root.querySelector('input[name="api_key"]'), "typed-key-0000");
    click(button(m.root, "Send a test message"));
    await settle();
    expect((m.root.querySelector('input[name="stream"]') as HTMLInputElement).value).toBe("typed-not-saved");
    expect((m.root.querySelector('input[name="api_key"]') as HTMLInputElement).value).toBe("typed-key-0000");
    expect(m.root.querySelector(".stat")!.textContent).toContain("Connected.");
    expect(m.root.querySelector("[data-tested]")!.textContent).toContain("saved");
  });

  it("says the test reaches the stream alone", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: { notifier: ZULIP } });
    expect(m.root.querySelector("[data-test-says]")!.textContent).toContain("does not try a direct message");
  });

  it("shows a failing direct message beside a stream that works", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: { notifier: { ...ZULIP, last_ok_at: "2026-11-06T10:00:00Z", direct_failure: "Zulip refused a message." } } });
    expect(m.root.querySelector(".stat")!.textContent).toContain("Connected.");
    expect(m.root.querySelector("[data-direct-failure]")!.textContent).toContain("A direct message failed. Zulip refused a message.");
  });

  it("refuses a login named twice, whatever its case, and sends nothing", async () => {
    const { calls, m } = await open({ [`GET ${ROUTE}`]: { notifier: ZULIP } });
    type(m.root.querySelector('textarea[name="people"]'), "Ana a@acme.dev\nana b@acme.dev");
    click(m.root.querySelector('button[type="submit"]'));
    await settle();
    expect(m.root.querySelector(".err")!.textContent).toContain("ana b@acme.dev");
    expect(calls.some(c => c.method === "PUT")).toBe(false);
  });

  it("keeps a password manager from filling the key", async () => {
    const { m } = await open({ [`GET ${ROUTE}`]: { notifier: null } });
    expect(m.root.querySelector('input[name="api_key"]')!.getAttribute("autocomplete")).toBe("new-password");
  });
});
