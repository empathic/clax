import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import ConfirmationCard from "./ConfirmationCard.svelte";
import { ConfirmationFeed, type ConfirmationView } from "./confirm.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const now = new Date("2026-10-29T10:01:00.000Z");
const ID = "cf_0123456789abcdef0123456789abcdef";
const HOSTILE = `<img src=x onerror="window.pwned=1">`;
const buttons = (root: Element) => [...root.querySelectorAll<HTMLButtonElement>(".acts button")];

function view(over: Partial<ConfirmationView> = {}): ConfirmationView {
  return {
    id: ID,
    status: "open",
    words: "push “Plan” to https://clax.dev",
    created_at: "2026-10-29T10:00:00.000Z",
    expires_at: "2026-10-29T10:10:00.000Z",
    confirm_url: `http://localhost:7480/inbox?confirm=${ID}`,
    agents: [
      { name: "ada", harness: "claude", handle: "a-7f3k", cwd: "/w/site", started_at: "2026-10-29T09:00:00.000Z" },
      { name: null, harness: "codex", handle: "a-9q2m", cwd: "", started_at: "2026-10-29T09:30:00.000Z" },
    ],
    ...over,
  };
}

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("ConfirmationCard", () => {
  it("asks to confirm the decision in words and lists the agents live now", () => {
    const m = mount(ConfirmationCard, { c: view(), now });
    const card = m.root.querySelector(".cfcard")!;
    expect(card.getAttribute("data-confirmation")).toBe(ID);
    expect(card.querySelector(".cwhat")!.textContent).toBe("Confirm: push “Plan” to https://clax.dev");
    expect([...card.querySelectorAll(".cagents li")].map(l => l.textContent)).toEqual(["ada in /w/site", "codex a-9q2m"]);
    expect(buttons(m.root).map(b => b.textContent!.trim())).toEqual(["Confirm", "Decline"]);
  });

  it("confirms or declines, then says which and drops its buttons", async () => {
    for (const [n, approve, status, said] of [[0, true, "approved", "Confirmed"], [1, false, "declined", "Declined"]] as const) {
      const calls = stubFetch({ [`POST /api/confirmations/${ID}`]: () => ({ confirmation: view({ status }) }) });
      const heard: ConfirmationView[] = [];
      const m = mount(ConfirmationCard, { c: view(), now, onanswered: (c: ConfirmationView) => heard.push(c) });
      click(buttons(m.root)[n]);
      await settle();
      expect(calls.filter(x => x.method === "POST")).toEqual([{ url: `/api/confirmations/${ID}`, method: "POST", body: { approve } }]);
      expect(m.root.querySelector(".outcome")!.textContent).toBe(said);
      expect(buttons(m.root)).toHaveLength(0);
      expect(heard.map(c => c.status)).toEqual([status]);
      m.unmount();
      vi.unstubAllGlobals();
    }
  });

  it("shows the daemon's refusal and keeps the buttons", async () => {
    stubFetch({ [`POST /api/confirmations/${ID}`]: [403, { error: { code: "browser_required", message: "a confirmation is answered in the owner's browser: open the inbox in Clax" } }] });
    const m = mount(ConfirmationCard, { c: view(), now });
    click(buttons(m.root)[0]);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("a confirmation is answered in the owner's browser: open the inbox in Clax");
    expect(buttons(m.root)).toHaveLength(2);
  });

  it("shows every word as text", () => {
    const m = mount(ConfirmationCard, { c: view({ words: HOSTILE, agents: [{ name: HOSTILE, harness: "claude", handle: "a-1", cwd: "", started_at: "2026-10-29T09:00:00.000Z" }] }), now });
    expect(m.root.querySelector("img")).toBeNull();
    expect(m.root.querySelector(".cwhat")!.textContent).toBe(`Confirm: ${HOSTILE}`);
  });

  it("says an answered or expired one is no longer waiting", () => {
    const m = mount(ConfirmationCard, { c: view({ status: "expired" }), now });
    expect(m.root.querySelector(".outcome")!.textContent).toBe("Expired: make the call again for a new one");
    expect(buttons(m.root)).toHaveLength(0);
  });
});

describe("ConfirmationFeed", () => {
  it("lists open ones as the stream tells, and fetches when the topic goes live", async () => {
    let on: ((e: { type: string; [k: string]: unknown }) => void) | undefined;
    const inbox = { hear: (f: NonNullable<typeof on>) => { on = f; return () => { on = undefined; }; } };
    const feed = new ConfirmationFeed({ list: async () => [view()] });
    feed.start(inbox);
    on!({ type: "ready" });
    await settle();
    expect(feed.open.map(c => c.id)).toEqual([ID]);
    const other = view({ id: "cf_ff", created_at: "2026-10-29T10:00:30.000Z" });
    on!({ type: "confirmation", confirmation: other });
    expect(feed.open.map(c => c.id)).toEqual([ID, "cf_ff"]);
    on!({ type: "confirmation", confirmation: view({ status: "approved" }) });
    expect(feed.open.map(c => c.id)).toEqual(["cf_ff"]);
  });

  it("stops for a browser that is not the owner's", async () => {
    let watching = false;
    const inbox = { hear: () => { watching = true; return () => { watching = false; }; } };
    const feed = new ConfirmationFeed({ list: async () => "forbidden" as const });
    feed.start(inbox);
    await feed.refresh();
    expect(watching).toBe(false);
    expect(feed.open).toEqual([]);
  });
});
