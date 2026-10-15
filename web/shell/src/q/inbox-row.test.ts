import { describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import InboxRow from "./InboxRow.svelte";
import { item } from "./fixtures";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const now = new Date("2026-10-07T09:17:03.120Z");

describe("InboxRow", () => {
  it("shows the title, text, age and a filled dot when unread", () => {
    const m = mount(InboxRow, { item: item("reply"), now, onOpen: vi.fn(), onToggle: vi.fn() });
    const li = m.root.querySelector("li")!;
    expect(li.hasAttribute("data-unread")).toBe(true);
    expect(m.root.querySelector(".title")!.textContent).toBe("claude replied on Quarterly Review");
    expect(m.root.querySelector(".text")!.textContent).toBe("Done: two columns now. The table sorts too.");
    expect(m.root.querySelector("time")!.textContent).toBe("5 min ago");
    expect(m.root.querySelector(".dot")!.getAttribute("aria-label")).toBe("Mark read");
    m.update({ item: item("reply", { read: true }), now, onOpen: vi.fn(), onToggle: vi.fn() });
    expect(li.hasAttribute("data-unread")).toBe(false);
    expect(m.root.querySelector(".dot")!.getAttribute("aria-label")).toBe("Mark unread");
    m.unmount();
  });

  it("tags an item on a linked thread with its remote, and names a remote reply's author there", () => {
    const m = mount(InboxRow, { item: item("reply", { agent: null, linked: { remote: "clax.dev", unlinked: false, author: "Ada (on clax.dev)" } }), now, onOpen: vi.fn(), onToggle: vi.fn() });
    expect(m.root.querySelector(".title")!.textContent).toBe("Ada (on clax.dev) replied on Quarterly Review clax.dev");
    expect(m.root.querySelector(".link-tag")!.textContent).toBe("clax.dev");
    m.update({ item: item("reply", { linked: { remote: "clax.dev", unlinked: true, author: null } }), now, onOpen: vi.fn(), onToggle: vi.fn() });
    expect(m.root.querySelector(".link-tag")!.textContent).toBe("clax.dev · unlinked");
    m.unmount();
  });

  it("links an ended agent's released threads below its row, and wraps its text", () => {
    const onOpen = vi.fn();
    const m = mount(InboxRow, { item: item("ended"), now, onOpen, onToggle: vi.fn() });
    const links = [...m.root.querySelectorAll(".released a")];
    expect(links.map(a => [a.textContent, a.getAttribute("href")])).toEqual([["Quarterly Review", "/a/7q3k9mzx2b4t?thread=01J9R0000000000000000000T1"]]);
    // A link is not inside the row's button.
    expect(links[0].closest("button")).toBeNull();
    expect(m.root.querySelector(".text")!.hasAttribute("data-wrap")).toBe(true);
    m.update({ item: item("reply"), now, onOpen, onToggle: vi.fn() });
    expect(m.root.querySelector(".released")).toBeNull();
    m.unmount();
  });

  it("keeps an undecided approval request's dot from marking it read, and lets a decided one be marked", () => {
    const onToggle = vi.fn();
    const open = item("approval");
    const m = mount(InboxRow, { item: open, now, onOpen: vi.fn(), onToggle });
    const dot = m.root.querySelector<HTMLButtonElement>(".dot")!;
    expect([dot.disabled, dot.getAttribute("aria-label")]).toEqual([true, "Unread until decided"]);
    click(dot);
    expect(onToggle).not.toHaveBeenCalled();
    m.update({ item: item("approval", { read: true }), now, onOpen: vi.fn(), onToggle });
    expect([dot.disabled, dot.getAttribute("aria-label")]).toEqual([false, "Mark unread"]);
    m.unmount();
  });

  it("opens on the row and toggles on the dot alone", () => {
    const onOpen = vi.fn();
    const onToggle = vi.fn();
    const it0 = item("version");
    const m = mount(InboxRow, { item: it0, now, onOpen, onToggle });
    click(m.root.querySelector(".title")!);
    expect(onOpen).toHaveBeenCalledWith(it0);
    click(m.root.querySelector(".dot")!);
    expect(onToggle).toHaveBeenCalledWith(it0);
    expect(onOpen).toHaveBeenCalledOnce();
    m.unmount();
  });

  it("says (deleted) for a gone source, and keeps hostile text as text", () => {
    const gone = item("reply", { gone: true, reply: { comment_id: "c", body: null, addressed: false } });
    const m = mount(InboxRow, { item: gone, now, onOpen: vi.fn(), onToggle: vi.fn() });
    expect(m.root.querySelector(".text")!.textContent).toBe("(deleted)");
    m.unmount();
    const evil = "<img src=x onerror=alert(1)>\u202eevil";
    const m2 = mount(InboxRow, { item: item("reply", { artifact: { id: "a", title: evil, kind: "html", page_url: null }, reply: { comment_id: "c", body: evil, addressed: false } }), now, onOpen: vi.fn(), onToggle: vi.fn() });
    expect(m2.root.querySelector("img")).toBeNull();
    expect(m2.root.querySelector(".title")!.textContent).toBe(`claude replied on ${evil}`);
    expect(m2.root.querySelector(".text")!.textContent).toBe(evil);
    m2.unmount();
  });
});
