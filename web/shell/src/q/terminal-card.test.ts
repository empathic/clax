import { beforeAll, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import { guardedAction, keyboardTrail } from "../view/trail";
import TerminalCard from "./TerminalCard.svelte";
import { view } from "./fixtures";
import { useTrail } from "./trail";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });

describe("TerminalCard", () => {
  beforeAll(() => useTrail({ keyboardTrail, guardedAction }));
  it("says the agent asks in the terminal, shows the headers, and offers Ask me here with the Esc note", async () => {
    const onPull = vi.fn(async () => {});
    const m = mount(TerminalCard, { q: view({ in_terminal: true, source: "hook" }), onPull });
    expect(m.root.querySelector(".asker")!.textContent).toBe("claude is asking you in the terminal");
    expect([...m.root.querySelectorAll(".hdrs li")].map(l => l.textContent)).toEqual(["Layout", "Sidebars", "Notes"]);
    expect(m.root.querySelector("input, textarea")).toBeNull();
    expect(m.root.querySelector(".esc")!.textContent).toContain("press Esc");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Ask me here")!);
    expect(onPull).toHaveBeenCalledOnce();
    m.unmount();
  });
  it("names its thread outside the thread's card, and not inside it", () => {
    const q = view({ in_terminal: true, thread: { id: "t1", open: true } });
    const out = mount(TerminalCard, { q, onPull: vi.fn(async () => {}) });
    const a = out.root.querySelector<HTMLAnchorElement>("a.about")!;
    expect([a.textContent, a.getAttribute("href")]).toEqual(["Quarterly Review · in a thread", "/a/7q3k9mzx2b4t?thread=t1"]);
    out.unmount();
    const inside = mount(TerminalCard, { q, here: "7q3k9mzx2b4t", thread: "t1", onPull: vi.fn(async () => {}) });
    expect(inside.root.querySelector("a.about")).toBeNull();
    inside.unmount();
  });
  it("has no Esc note for a question asked in chat", () => {
    const m = mount(TerminalCard, { q: view({ in_terminal: true, source: "ask" }), onPull: vi.fn(async () => {}) });
    expect(m.root.querySelector(".esc")).toBeNull();
    m.unmount();
  });
  it("says Ask me here was not sent when it fails, and what closed it once closed", async () => {
    const m = mount(TerminalCard, { q: view({ in_terminal: true }), onPull: vi.fn(async () => { throw new Error("down"); }) });
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Ask me here")!);
    await vi.waitFor(() => { flush(); expect(m.root.querySelector(".err")?.textContent).toBe("Not sent: try again"); });
    m.unmount();
    const c = mount(TerminalCard, { q: view({ in_terminal: true, status: "withdrawn", pulled: true }), onPull: vi.fn(async () => {}) });
    expect(c.root.querySelector(".closed")!.textContent).toBe("Sent to claude: it asks here next");
    expect(c.root.querySelector("button")).toBeNull();
    c.unmount();
  });
});
