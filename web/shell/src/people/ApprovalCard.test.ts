import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { approvalCard, item } from "../q/fixtures";
import { flush, mount } from "../test/svelte";
import ApprovalCard from "./ApprovalCard.svelte";
import { settle, stubFetch } from "./fixtures";
import { loadMarkdown } from "../view/md-load.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const HOSTILE = `<img src=x onerror="window.pwned=1">`;
const now = new Date("2026-10-07T09:17:03.120Z");
const RID = "01J9R0000000000000000000AA";
const buttons = (root: Element) => [...root.querySelectorAll<HTMLButtonElement>(".acts button")];

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("ApprovalCard", () => {
  it("shows the author, the agent, the page and thread, and the withheld content", () => {
    const m = mount(ApprovalCard, { item: item("approval"), now });
    const card = m.root.querySelector(".acard")!;
    expect(card.getAttribute("data-approval")).toBe(RID);
    expect(card.querySelector(".ahead")!.textContent).toContain("Mia wants to reach your agent clax-multi-person");
    const what = card.querySelector(".what")!;
    expect(what.textContent).toContain("1 comment on");
    expect(what.querySelector("a")!.textContent).toBe("Quarterly Review");
    expect(what.querySelector("a")!.getAttribute("href")).toBe("/a/7q3k9mzx2b4t?thread=01J9T0000000000000000000AA");
    expect(what.textContent).toContain("«Quarterly goals»");
    expect(card.querySelector(".held")!.textContent).toContain("The goals table needs a Q4 column.");
  });

  it("offers the four scopes, the thread's saying it includes later replies", () => {
    const m = mount(ApprovalCard, { item: item("approval"), now });
    expect(buttons(m.root).map(b => b.textContent!.trim())).toEqual([
      "Allow this thread, including later replies", "Always allow Mia on this page", "Always allow Mia everywhere", "Decline",
    ]);
  });

  it("decides each button's own scope, then says what was decided", async () => {
    for (const [n, scope, said] of [[0, "thread", "Allowed in this thread"], [1, "artifact", "Allowed on this page"], [2, "everywhere", "Allowed everywhere"], [3, "decline", "Declined"]] as const) {
      const calls = stubFetch({ [`POST /api/approvals/requests/${RID}/decide`]: () => ({ request: { ...approvalCard(), status: scope === "decline" ? "declined" : "approved", scope: scope === "decline" ? null : scope }, approval: null }) });
      const m = mount(ApprovalCard, { item: item("approval"), now });
      click(buttons(m.root)[n]);
      await settle();
      expect(calls).toEqual([{ url: `/api/approvals/requests/${RID}/decide`, method: "POST", body: { scope } }]);
      expect(m.root.querySelector(".outcome")!.textContent).toContain(said);
      expect(buttons(m.root)).toHaveLength(0);
      m.unmount();
      vi.unstubAllGlobals();
    }
  });

  it("shows the daemon's refusal and keeps the buttons", async () => {
    stubFetch({ [`POST /api/approvals/requests/${RID}/decide`]: [409, { error: { code: "request_closed", message: "The request is no longer open." } }] });
    const m = mount(ApprovalCard, { item: item("approval"), now });
    click(buttons(m.root)[0]);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("The request is no longer open.");
    expect(buttons(m.root)).toHaveLength(4);
  });

  it("keeps names, titles and content as text, never markup", () => {
    const card = approvalCard({ author: { public_id: "p4", name: `${HOSTILE}Mia` }, agent: { handle: "a", name: `${HOSTILE}bot`, harness: "codex", live: true },
      content: { thread: { id: "t", status: "open" }, comments: [{ id: "c", body: `${HOSTILE}body`, created_at: "", author_name: `${HOSTILE}Mia`, by_agent: false }] },
      via_page: { authors: [{ public_id: "p5", name: `${HOSTILE}Pub` }], viewer: { public_id: "p6", name: `${HOSTILE}View` } } });
    const m = mount(ApprovalCard, { item: item("approval", { approval: card, artifact: { id: "7q3k9mzx2b4t", title: `${HOSTILE}Review`, kind: "html", page_url: null } }), now });
    expect(m.root.querySelector("img")).toBeNull();
    expect(m.root.textContent).toContain(`${HOSTILE}Mia wants to reach your agent ${HOSTILE}bot`);
    expect(m.root.querySelector(".via")!.textContent).toContain(`${HOSTILE}Pub's page, via ${HOSTILE}View's browser`);
    expect(m.root.querySelector(".held")!.textContent).toContain(`${HOSTILE}body`);
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
  });

  it("shows the first 3 lines of the content, then Show all", () => {
    const body = ["one", "two", "three", "four", "five"].join("\n");
    const card = approvalCard({ content: { thread: { id: "t", status: "open" }, comments: [{ id: "c", body, created_at: "", author_name: "Mia", by_agent: false }] } });
    const m = mount(ApprovalCard, { item: item("approval", { approval: card }), now });
    const held = m.root.querySelector(".held")!;
    expect(held.hasAttribute("data-open")).toBe(false);
    expect(held.textContent).not.toContain("four");
    const more = m.root.querySelector<HTMLButtonElement>(".show")!;
    expect(more.textContent).toBe("Show all");
    expect(more.getAttribute("aria-expanded")).toBe("false");
    click(more);
    expect(held.textContent).toContain("five");
    expect(more.textContent).toBe("Show less");
    // Short content has no Show all.
    m.unmount();
    expect(mount(ApprovalCard, { item: item("approval"), now }).root.querySelector(".show")).toBeNull();
  });

  it("offers Show all when 3 lines wrap past the clamp, measured from the laid-out box, and again on resize", () => {
    // jsdom lays nothing out: the clamped text's box is stubbed, overflowing by `over` px.
    let over = 0;
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get(this: HTMLElement) { return this.classList.contains("htext") ? 57 + over : 0; } });
    Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get(this: HTMLElement) { return this.classList.contains("htext") ? 57 : 0; } });
    try {
      const line = (c: string) => c.repeat(145);
      const body = [line("a"), line("b"), line("c")].join("\r\n");
      over = 38;
      const card = approvalCard({ content: { thread: { id: "t", status: "open" }, comments: [{ id: "c", body, created_at: "", author_name: "Mia", by_agent: false }] } });
      const m = mount(ApprovalCard, { item: item("approval", { approval: card }), now });
      const more = m.root.querySelector<HTMLButtonElement>(".show");
      expect(more?.textContent).toBe("Show all");
      // Lines split on CRLF keep no carriage return.
      expect(m.root.querySelector(".held")!.textContent).not.toContain("\r");
      m.unmount();
      // Not clipped at first; the page narrows, the lines wrap, and Show all appears.
      over = 0;
      const n = mount(ApprovalCard, { item: item("approval", { approval: card }), now });
      expect(n.root.querySelector(".show")).toBeNull();
      over = 38;
      flush(() => { dispatchEvent(new Event("resize")); });
      expect(n.root.querySelector(".show")?.textContent).toBe("Show all");
    } finally {
      // The prototype's own stubs go; Element's getters show through again.
      delete (HTMLElement.prototype as { scrollHeight?: number }).scrollHeight;
      delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
    }
  });

  it("leaves out the thread button outside threads, and names a deleted page as one page", () => {
    const card = approvalCard({ thread_id: null, count: 2, content: { title: "Quarterly Review", description: "Goals", files: ["index.html"], docs: ["rows/1"], assets: [] } });
    const m = mount(ApprovalCard, { item: item("approval", { approval: card, thread: null, artifact: { id: "7q3k9mzx2b4t", title: null, kind: null, page_url: null } }), now });
    expect(buttons(m.root).map(b => b.textContent!.trim())).toEqual(["Always allow Mia on this page", "Always allow Mia everywhere", "Decline"]);
    expect(m.root.querySelector(".what")!.textContent).toContain("on one page");
    expect(m.root.querySelector(".held")!.textContent).toContain("index.html");
  });

  it("names the owner as such, says a decided request's outcome, and a gone one's", () => {
    const owner = approvalCard({ author: { public_id: "o1", name: "Alex", owner: true }, status: "approved", scope: "everywhere" });
    const m = mount(ApprovalCard, { item: item("approval", { approval: owner, read: true }), now });
    expect(m.root.querySelector(".ahead")!.textContent).toContain("Alex (the owner) wants to reach");
    expect(m.root.querySelector(".outcome")!.textContent).toContain("Allowed everywhere");
    m.unmount();
    // An unnamed owner, within a sentence; their comment under their name, not the stored one.
    const unnamed = approvalCard({ author: { public_id: "o1", name: "The owner", owner: true },
      content: { thread: { id: "t", status: "open" }, comments: [{ id: "c", body: "Q4 please", created_at: "", author_name: "Viewer", by_agent: false }] } });
    const u = mount(ApprovalCard, { item: item("approval", { approval: unnamed }), now });
    expect(buttons(u.root).map(b => b.textContent!.trim())).toContain("Always allow the owner everywhere");
    expect(u.root.querySelector(".held")!.textContent).toBe("The owner: Q4 please");
    u.unmount();
    const gone = mount(ApprovalCard, { item: item("approval", { approval: { id: RID }, gone: true }), now });
    expect(gone.root.querySelector(".outcome")!.textContent).toContain("no longer waits");
    expect(buttons(gone.root)).toHaveLength(0);
  });
});

// Last in the file: once the Markdown module is loaded, every card after renders with it.
describe("ApprovalCard's comments as Markdown", () => {
  const comment = (body: string) => approvalCard({ content: { thread: { id: "t", status: "open" }, comments: [{ id: "c", body, created_at: "", author_name: "Mia", by_agent: false }] } });
  it("renders each withheld comment as Markdown under its author, hiding no word of it", async () => {
    await loadMarkdown();
    const m = mount(ApprovalCard, { item: item("approval", { approval: comment("Add a **Q4** column to `goals`, see [the doc](https://example.com/q4).") }), now });
    flush();
    const part = m.root.querySelector(".held .hpart")!;
    expect(part.querySelector(".hwho")!.textContent).toBe("Mia: ");
    expect(part.querySelector(".md strong")!.textContent).toBe("Q4");
    expect(part.querySelector(".md code")!.textContent).toBe("goals");
    expect(part.querySelector(".md a")!.getAttribute("href")).toBe("https://example.com/q4");
    expect(part.textContent).toBe("Mia: Add a Q4 column to goals, see the doc.");
    m.unmount();
    // What a reading would leave out shows: a reference definition, raw HTML, an image's address.
    const hidden = "Approve this[x]\n\n[x]: https://example.com \"secret words\"\n\n<span hidden>tucked</span> ![alt](https://example.com/i.png)";
    const h = mount(ApprovalCard, { item: item("approval", { approval: comment(hidden) }), now });
    flush();
    click(h.root.querySelector<HTMLButtonElement>(".show")!);
    const text = h.root.querySelector(".held")!.textContent!;
    for (const word of ["secret words", "tucked", "<span hidden>", "alt"]) expect(text, word).toContain(word);
    expect(h.root.querySelector(".held img, .held span[hidden]")).toBeNull();
  });

  it("collapses to the first 3 lines of the comments, each comment led by its author", async () => {
    await loadMarkdown();
    const card = approvalCard({ content: { thread: { id: "t", status: "open" }, comments: [
      { id: "c1", body: "- one\n- two", created_at: "", author_name: "Mia", by_agent: false },
      { id: "c2", body: "three\n\nfour", created_at: "", author_name: "bot", by_agent: true },
    ] } });
    const m = mount(ApprovalCard, { item: item("approval", { approval: card }), now });
    flush();
    const held = m.root.querySelector(".held")!;
    expect([...held.querySelectorAll(".hwho")].map(w => w.textContent)).toEqual(["Mia: ", "bot (Mia's agent): "]);
    expect([...held.querySelectorAll(".hpart:first-child li")].map(li => li.textContent)).toEqual(["one", "two"]);
    expect(held.textContent).toContain("three");
    expect(held.textContent).not.toContain("four");
    click(m.root.querySelector<HTMLButtonElement>(".show")!);
    expect(held.textContent).toContain("four");
  });
});
