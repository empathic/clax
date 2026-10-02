import { describe, expect, it, vi } from "vitest";
import Sidebar from "./ui/Sidebar.svelte";
import { flush, mount } from "./test/svelte";
import type { Thread } from "./threads";

const base = { artifact_id: "7q3k9mzx2b4t", version_n: 1, has_clip: false, clip_url: null, created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null };
const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const comment = (id: string, kind: "viewer" | "agent", name: string, body: string) => ({ id, thread_id: "t", author_kind: kind, author_name: name, via_harness: kind === "agent" ? name : null, body, created_at: base.created_at });

describe("Sidebar", () => {
  it("groups open, detached, and resolved threads and labels agent comments", () => {
    const threads: Thread[] = [
      { ...base, id: "a", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "two columns"), comment("2", "agent", "claude", "done")],
        feedback_state: { thread_id: "a", state: "acknowledged", tier: "wait", since: base.created_at, resends: 0, exhausted: false } },
      { ...base, id: "b", anchor, status: "open", sent_to_agent: false, comments: [comment("3", "viewer", "Viewer", "gone")] },
      { ...base, id: "c", anchor, status: "resolved", sent_to_agent: false, comments: [comment("4", "viewer", "Viewer", "old")] },
    ];
    const found = { a: { id: "a", found: true, method: "exact" as const, rect: null }, b: { id: "b", found: false, method: null, rect: null } };
    const root = document.createElement("div");
    document.body.appendChild(root);
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads, resolved: found, now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() }, root);
    expect(root.querySelectorAll(".section-open .thread-card")).toHaveLength(1);
    expect(root.querySelectorAll(".section-detached .thread-card")).toHaveLength(1);
    expect(root.querySelectorAll(".section-resolved .thread-card")).toHaveLength(1);
    expect(root.querySelector(".msg.agent .author")!.textContent).toBe("claude");
    expect(root.querySelector(".section-open .waiting")!.textContent).toBe("seen by the agent");
    // Only thread b is open and unsent; a was sent and c is resolved.
    expect(Array.from(root.querySelectorAll("button")).filter(b => b.textContent === "Send to claude")).toHaveLength(1);
    view.unmount();
    root.remove();
  });

  it("says which comments the page wrote", () => {
    const t: Thread = { ...base, id: "p", anchor, status: "open", sent_to_agent: false, comments: [{ ...comment("1", "viewer", "Alex", "from the page"), via_page: true }, comment("2", "viewer", "Alex", "by hand")] };
    const root = document.createElement("div");
    document.body.appendChild(root);
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() }, root);
    expect(Array.from(root.querySelectorAll(".msg .author"), a => a.textContent)).toEqual(["Alex · via the page", "Alex"]);
    view.unmount();
    root.remove();
  });

  it("names who resolved a thread in its history without exposing identifiers", () => {
    const me = { public_id: "u_0123456789abcdef012345", display_name: "Alex", created_at: base.created_at };
    const resolved = (id: string, by: string): Thread => ({ ...base, id, anchor, status: "resolved", sent_to_agent: false, resolved_at: base.created_at, resolved_by: by, comments: [comment(id, "viewer", "Viewer", "x")] });
    const threads = [
      resolved("a", "viewer:u_0123456789abcdef012345"),
      resolved("b", "viewer:u_ffffffffffffffffffffff"),
      resolved("c", "viewer:anonymous"),
      resolved("d", "agent:codex"),
      { ...resolved("e", "viewer:u_eeeeeeeeeeeeeeeeeeeeee"), resolved_by_name: "Mia" },
    ];
    const root = document.createElement("div");
    document.body.appendChild(root);
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads, resolved: {}, me, now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() }, root);
    expect(Array.from(root.querySelectorAll(".hist .ev:last-child")).map(e => e.textContent)).toEqual([
      " · v1 Alex resolved", " · v1 Viewer resolved", " · v1 Viewer resolved", " · codex resolved", " · v1 Mia resolved",
    ]);
    for (const h of root.querySelectorAll(".hist")) expect(h.textContent).not.toContain("u_");
    view.unmount();
    root.remove();
  });

  it("labels threads on other pages and never counts them as detached here", () => {
    const onAbout = { ...anchor, file: "about.html" };
    const threads: Thread[] = [
      { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "here")] },
      { ...base, id: "b", anchor: onAbout, status: "open", sent_to_agent: false, comments: [comment("2", "viewer", "Alex", "there")] },
      { ...base, id: "c", anchor: onAbout, status: "resolved", sent_to_agent: false, comments: [comment("3", "viewer", "Alex", "done there")] },
    ];
    // A stale result from another page must not detach b.
    const found = { a: { id: "a", found: false, method: null, rect: null }, b: { id: "b", found: false, method: null, rect: null } };
    const root = document.createElement("div");
    document.body.appendChild(root);
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads, resolved: found, file: "index.html", now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() }, root);
    expect(Array.from(root.querySelectorAll(".section-detached .thread-card")).map(c => c.getAttribute("data-thread"))).toEqual(["a"]);
    const open = Array.from(root.querySelectorAll<HTMLElement>(".section-open .thread-card"));
    expect(open.map(c => c.getAttribute("data-thread"))).toEqual(["b"]);
    expect(open[0].querySelector(".thread-num")).toBeNull();
    expect(open[0].querySelector(".file-label")!.textContent).toBe("on about.html");
    expect(root.querySelector('[data-thread="c"] .file-label')!.textContent).toBe("on about.html");
    expect(root.querySelector('[data-thread="a"] .file-label')).toBeNull();
    view.unmount();
    root.remove();
  });

  it("lists a thread on a page the version does not hold under Detached", () => {
    const threads: Thread[] = [
      { ...base, id: "g", anchor: { ...anchor, file: "gone.html" }, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "gone page")] },
      { ...base, id: "b", anchor: { ...anchor, file: "about.html" }, status: "open", sent_to_agent: false, comments: [comment("2", "viewer", "Alex", "there")] },
    ];
    const root = document.createElement("div");
    document.body.appendChild(root);
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads, resolved: {}, file: "index.html", holds: (f: string) => f !== "gone.html", now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() }, root);
    expect(Array.from(root.querySelectorAll(".section-detached .thread-card")).map(c => c.getAttribute("data-thread"))).toEqual(["g"]);
    expect(Array.from(root.querySelectorAll(".section-open .thread-card")).map(c => c.getAttribute("data-thread"))).toEqual(["b"]);
    view.unmount();
    root.remove();
  });

  it("selects a thread from its keyboard-reachable header button", () => {
    const onSelect = vi.fn();
    const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "note")] };
    const root = document.createElement("div");
    document.body.appendChild(root);
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, now: new Date(base.created_at), selected: null, onSelect, onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() }, root);
    const head = root.querySelector<HTMLButtonElement>(".thread-card button.card-head")!;
    expect(head.type).toBe("button");
    head.click();
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith(t);
    view.unmount();
    root.remove();
  });

  it("posts a reply on Cmd+Enter or Ctrl+Enter, once, without selecting the thread", () => {
    const onReply = vi.fn();
    const onSelect = vi.fn();
    const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "note")] };
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, now: new Date(base.created_at), selected: null, onSelect, onSend: vi.fn(), onResolve: vi.fn(), onReply });
    const input = view.root.querySelector<HTMLInputElement>('input[aria-label="Reply"]')!;
    const key = (init: KeyboardEventInit) => flush(() => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...init })));
    flush(() => input.click());
    flush(() => { input.value = "  "; input.dispatchEvent(new Event("input", { bubbles: true })); });
    key({ metaKey: true });
    expect(onReply).not.toHaveBeenCalled();
    flush(() => { input.value = "looks good"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    key({});
    expect(onReply).not.toHaveBeenCalled();
    key({ metaKey: true });
    expect(onReply).toHaveBeenCalledExactlyOnceWith(t, "looks good");
    expect(input.value).toBe("");
    flush(() => { input.value = "again"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    key({ ctrlKey: true });
    expect(onReply).toHaveBeenLastCalledWith(t, "again");
    expect(onSelect).not.toHaveBeenCalled();
    view.unmount();
  });

  it("reads each thread's history as version-tagged events, and tags a thread whose element changed in a later version", () => {
    const v = (n: number, at: string) => ({ artifact_id: base.artifact_id, n, label: null, created_at: at, files: {} });
    const versions = [v(1, "2026-09-29T09:00:00.000Z"), v(2, "2026-09-29T11:00:00.000Z")];
    const t: Thread = { ...base, id: "a", anchor: { ...anchor, html_hash: "h1" }, status: "open", sent_to_agent: false,
      comments: [comment("1", "viewer", "alex", "two columns"), { ...comment("2", "viewer", "Mia", "agreed"), created_at: "2026-09-29T10:30:00.000Z" }] };
    const found = { a: { id: "a", found: true, method: "selector" as const, rect: null } };
    const props = { versions, shown: 2, agent: "claude", threads: [t], resolved: found, now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() };
    const view = mount(Sidebar, props);
    // A list named History, its events separated by a "·" that is read and copied.
    const hist = view.root.querySelector("ul.hist")!;
    expect(hist.getAttribute("aria-label")).toBe("History");
    expect(Array.from(hist.querySelectorAll("li.ev"), e => e.textContent)).toEqual(["v1 alex commented", " · v1 Mia replied"]);
    expect(hist.textContent).toBe("v1 alex commented · v1 Mia replied");
    expect(view.root.querySelector(".vt.out")!.textContent).toBe("outdated");
    view.update({ ...props, shown: 1 });
    expect(view.root.querySelector(".vt.out")).toBeNull();
    view.unmount();
  });

  it("heads every group, and opens a collapsed group that holds the selected card or the card just resolved", () => {
    const threads: Thread[] = [
      { ...base, id: "o", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "open")] },
      { ...base, id: "d", anchor: { ...anchor, file: "gone.html" }, status: "open", sent_to_agent: false, comments: [comment("2", "viewer", "Alex", "gone")] },
      { ...base, id: "r", anchor, status: "resolved", sent_to_agent: false, comments: [comment("3", "viewer", "Alex", "done")] },
    ];
    const props = { versions: [], shown: 1, agent: "claude", threads, resolved: {}, file: "index.html", holds: (f: string) => f !== "gone.html", now: new Date(base.created_at), selected: null as string | null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() };
    const view = mount(Sidebar, props);
    expect(Array.from(view.root.querySelectorAll("h2"), h => h.querySelector(".t")!.textContent)).toEqual(["Open", "Detached", "Resolved"]);
    const group = (cls: string) => view.root.querySelector<HTMLDetailsElement>(`details.${cls}`)!;
    expect([group("section-detached").open, group("section-resolved").open]).toEqual([false, false]);
    view.update({ ...props, selected: "d" });
    expect(group("section-detached").open).toBe(true);
    // The viewer collapses it: a thread update with the same card selected leaves it collapsed.
    flush(() => { group("section-detached").open = false; group("section-detached").dispatchEvent(new Event("toggle")); });
    view.update({ ...props, selected: "d", threads: threads.map(x => ({ ...x })) });
    expect(group("section-detached").open).toBe(false);
    expect(group("section-detached").getAttribute("aria-label")).toBe("Detached 1");
    view.update({ ...props, selected: "r" });
    expect(group("section-resolved").open).toBe(true);
    view.unmount();
    // Resolve on an open card: once the card is under Resolved, that group opens.
    const second = mount(Sidebar, props);
    const g = (cls: string) => second.root.querySelector<HTMLDetailsElement>(`details.${cls}`)!;
    flush(() => Array.from(second.root.querySelectorAll<HTMLButtonElement>('[data-thread="o"] button')).find(b => b.textContent === "Resolve")!.click());
    expect(props.onResolve).toHaveBeenCalledWith(threads[0]);
    expect(g("section-resolved").open).toBe(false);
    second.update({ ...props, threads: [{ ...threads[0], status: "resolved" }, threads[1], threads[2]] });
    expect(g("section-resolved").open).toBe(true);
    second.unmount();
  });

  it("refuses Send and Resolve from the keyboard on a tainted trail, says how to act, and takes a click", async () => {
    const { keyboardTrail } = await import("./view/trail");
    const onSend = vi.fn();
    const onResolve = vi.fn();
    const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "note")] };
    const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend, onResolve, onReply: vi.fn() });
    const button = (name: string) => Array.from(view.root.querySelectorAll("button")).find(b => b.textContent === name)!;
    const hint = view.root.querySelector(".act-hint")!;
    expect(hint.getAttribute("role")).toBe("status");
    keyboardTrail.taint();
    try {
      // Enter or Space on a button is a click with detail 0.
      flush(() => button("Send to claude").dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 0 })));
      expect(onSend).not.toHaveBeenCalled();
      expect(hint.textContent).toBe("Click to send, or press Esc first");
      flush(() => button("Resolve").dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 0 })));
      expect(onResolve).not.toHaveBeenCalled();
      expect(hint.textContent).toBe("Click to resolve, or press Esc first");
      flush(() => button("Send to claude").dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 })));
      expect(onSend).toHaveBeenCalledOnce();
      expect(hint.textContent).toBe("");
      keyboardTrail.clear();
      flush(() => button("Resolve").dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 0 })));
      expect(onResolve).toHaveBeenCalledOnce();
    } finally {
      keyboardTrail.clear();
      view.unmount();
    }
  });

  it("keeps each card's time current while nothing is waiting", () => {
    vi.useFakeTimers({ now: new Date(base.created_at) });
    try {
      const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "note")] };
      const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
      const when = () => view.root.querySelector(".card-head")!.lastElementChild!.textContent;
      expect(when()).toBe("just now");
      flush(() => vi.advanceTimersByTime(5 * 60_000));
      expect(when()).toBe("5 min ago");
      view.unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("ticks the elapsed time of a thread waiting for the agent each second", () => {
    vi.useFakeTimers({ now: new Date(base.created_at) });
    try {
      const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "note")],
        feedback_state: { thread_id: "a", state: "sent", tier: "stop_hook", since: base.created_at, resends: 0, exhausted: false } };
      const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
      const waiting = () => view.root.querySelector(".waiting")!.textContent;
      expect(waiting()).toContain("· 0 s ·");
      flush(() => vi.advanceTimersByTime(2000));
      expect(waiting()).toContain("· 2 s ·");
      view.unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("marks a thread a working record names, with its clock and a history event, and shows the waiting line again when the record drops it", () => {
    const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "note")],
      feedback_state: { thread_id: "a", state: "sent", tier: "stop_hook", since: base.created_at, resends: 0, exhausted: false } };
    const w = { key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: ["a"], started_at: base.created_at, last_heartbeat: base.created_at };
    const props = { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, now: new Date("2026-09-29T10:00:42.000Z"), selected: null,
      agents: [{ handle: "a_1111aaaa", harness: "claude", live: true }], onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() };
    const view = mount(Sidebar, { ...props, working: [w] });
    const st = view.root.querySelector(".thread-card .st.ag")!;
    expect(st.firstChild!.nextSibling!.textContent).toBe("claude is working on it");
    expect(st.querySelector("small")!.textContent).toBe("0:42");
    expect(view.root.querySelector(".thread-card .waiting")).toBeNull();
    expect(view.root.querySelector(".hist")!.textContent).toContain("claude working on it");
    view.update({ ...props, working: [] });
    expect(view.root.querySelector(".thread-card .st.ag")).toBeNull();
    expect(view.root.querySelector(".thread-card .waiting")).not.toBeNull();
    expect(view.root.querySelector(".hist")?.textContent ?? "").not.toContain("working on it");
    view.unmount();
  });
});
