import { describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../bridge/test/trusted";
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

  it("leads with the threads the latest version addressed, apart from Open, and labels the agent's reply with that version", async () => {
    const versions = [
      { artifact_id: base.artifact_id, n: 1, label: null, created_at: "2026-09-29T09:00:00.000Z", files: {} },
      { artifact_id: base.artifact_id, n: 2, label: null, created_at: "2026-09-29T11:00:00.000Z", files: {}, addresses: ["a"], agent: "a_1", agent_harness: "claude" },
    ];
    const threads: Thread[] = [
      { ...base, id: "a", anchor, status: "open", sent_to_agent: true, addressed_in: [2], comments: [comment("1", "viewer", "Alex", "two columns"), comment("2", "agent", "claude", "done")] },
      { ...base, id: "b", anchor, status: "open", sent_to_agent: false, comments: [comment("3", "viewer", "Alex", "units")] },
    ];
    const root = document.createElement("div");
    document.body.appendChild(root);
    const view = mount(Sidebar, { versions, shown: 2, agent: "claude", threads, resolved: {}, now: new Date(base.created_at), selected: null, decided: { n: 2, ids: ["a"], dot: true, line: "v2 addressed 1" },
      onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() }, root);
    await vi.waitFor(() => expect(root.querySelector(".section-addressed")).not.toBeNull());
    flush();
    expect(root.querySelector(".section-addressed h2")!.textContent).toContain("Addressed in v2");
    expect(Array.from(root.querySelectorAll(".section-addressed .thread-card"), c => c.getAttribute("data-thread"))).toEqual(["a"]);
    expect(Array.from(root.querySelectorAll(".section-open .thread-card"), c => c.getAttribute("data-thread"))).toEqual(["b"]);
    expect(root.querySelector(".section-addressed .msg.agent .author")!.textContent).toBe("claude · addressed in v2");
    expect(root.querySelector(".section-addressed .hist")!.textContent).toContain("v2 claude addressed it");
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
      "Alex resolved", "Viewer resolved", "Viewer resolved", "codex resolved", "Mia resolved",
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

  it("posts a reply on Enter, Cmd+Enter or Ctrl+Enter, once, without selecting the thread", () => {
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
    // Shift+Enter is not a submit.
    key({ shiftKey: true });
    expect(onReply).not.toHaveBeenCalled();
    key({ metaKey: true });
    expect(onReply).toHaveBeenCalledExactlyOnceWith(t, "looks good");
    expect(input.value).toBe("");
    flush(() => { input.value = "plain"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    key({});
    expect(onReply).toHaveBeenLastCalledWith(t, "plain");
    flush(() => { input.value = "again"; input.dispatchEvent(new Event("input", { bubbles: true })); });
    key({ ctrlKey: true });
    expect(onReply).toHaveBeenLastCalledWith(t, "again");
    expect(onSelect).not.toHaveBeenCalled();
    view.unmount();
  });

  it("tags each comment with its author, version and time, keeps the history to what the comments do not say, and tags a thread whose element changed in a later version", () => {
    const v = (n: number, at: string) => ({ artifact_id: base.artifact_id, n, label: null, created_at: at, files: {} });
    const versions = [v(1, "2026-09-29T09:00:00.000Z"), v(2, "2026-09-29T11:00:00.000Z")];
    const t: Thread = { ...base, id: "a", anchor: { ...anchor, html_hash: "h1" }, status: "open", sent_to_agent: false,
      comments: [comment("1", "viewer", "alex", "two columns"), { ...comment("2", "viewer", "Mia", "agreed"), created_at: "2026-09-29T10:30:00.000Z" }] };
    const found = { a: { id: "a", found: true, method: "selector" as const, rect: null } };
    const props = { versions, shown: 2, agent: "claude", threads: [t], resolved: found, now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() };
    const view = mount(Sidebar, props);
    // Each comment: its author, the version it was made on or current then, and its time.
    expect(Array.from(view.root.querySelectorAll(".msg .by"), e => e.textContent)).toEqual(["alexv1just now", "Miav1just now"]);
    const time = view.root.querySelector(".msg .by time")!;
    expect(time.getAttribute("datetime")).toBe(base.created_at);
    // The history repeats none of them: with only comments, there is none.
    expect(view.root.querySelector("ul.hist")).toBeNull();
    view.update({ ...props, threads: [{ ...t, sends: [{ batch_id: "b", size: 1, note: null, sent_by: "alex", sent_at: "2026-09-29T11:30:00.000Z" }] }] });
    // A list named History, its events separated by a "·" that is read and copied.
    const hist = view.root.querySelector("ul.hist")!;
    expect(hist.getAttribute("aria-label")).toBe("History");
    expect(hist.textContent).toBe("v2 alex sent it");
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

  it("on a tainted trail, takes Send, Resolve and Reply only from a pointer's click, says so until the trail clears, and keeps focus on the card after a Send", async () => {
    const { keyboardTrail } = await import("./view/trail");
    const onSend = vi.fn();
    const onResolve = vi.fn();
    const onReply = vi.fn();
    const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "note")] };
    const props = { versions: [], shown: 1, agent: "claude", threads: [t], resolved: {}, now: new Date(base.created_at), selected: null, onSelect: vi.fn(), onSend, onResolve, onReply };
    const view = mount(Sidebar, props);
    const button = (name: string) => Array.from(view.root.querySelectorAll("button")).find(b => b.textContent === name)!;
    const hint = view.root.querySelector(".thread-card .act-hint")!;
    const click = (el: Element, detail: number) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail })); });
    const input = view.root.querySelector<HTMLInputElement>(".reply input")!;
    const typeReply = (text: string) => flush(() => { input.value = text; input.dispatchEvent(new Event("input", { bubbles: true })); });
    const key = (el: Element, k: string, mods: KeyboardEventInit = {}) => flush(() => { dispatchTrusted(el, new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...mods })); });
    expect(hint.getAttribute("role")).toBe("status");
    keyboardTrail.taint();
    try {
      // Enter or Space on a button is a click with detail 0.
      click(button("Send to claude"), 0);
      expect(onSend).not.toHaveBeenCalled();
      expect(hint.textContent).toBe("Click to send");
      click(button("Resolve"), 0);
      expect(onResolve).not.toHaveBeenCalled();
      expect(hint.textContent).toBe("Click to resolve");
      // Typing meant for the page that lands in Reply is never posted by a key.
      typeReply("pwd hunter2");
      key(input, "Enter");
      key(input, "Enter", { metaKey: true });
      key(input, "Enter", { ctrlKey: true });
      click(button("Reply"), 0);
      expect(onReply).not.toHaveBeenCalled();
      expect(hint.textContent).toBe("Click to reply");
      // An Escape clears nothing.
      key(input, "Escape");
      click(button("Reply"), 0);
      expect(onReply).not.toHaveBeenCalled();
      // A pointer's click acts.
      click(button("Reply"), 1);
      expect(onReply).toHaveBeenCalledWith(t, "pwd hunter2");
      expect(hint.textContent).toBe("");
      click(button("Send to claude"), 0);
      expect(hint.textContent).toBe("Click to send");
      // The hint goes once the trail clears; then keys act again.
      flush(() => keyboardTrail.clear());
      expect(hint.textContent).toBe("");
      typeReply("ok");
      key(input, "Enter");
      expect(onReply).toHaveBeenLastCalledWith(t, "ok");
      // A Send from the keyboard moves focus to the card's head before Send goes.
      button("Send to claude").focus();
      click(button("Send to claude"), 0);
      expect(onSend).toHaveBeenCalledOnce();
      view.update({ ...props, threads: [{ ...t, sent_to_agent: true }] });
      expect(document.activeElement).toBe(view.root.querySelector(".card-head"));
      click(button("Resolve"), 0);
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

  describe("batch send", () => {
    const open = (id: string, quote: string): Thread => ({ ...base, id, anchor: { ...anchor, quote }, status: "open", sent_to_agent: false, comments: [comment(`${id}1`, "viewer", "Alex", quote)] });
    const threads = [open("a", "Goals"), open("b", "Units"), { ...open("c", "Old"), status: "resolved" as const }];
    const two = [{ handle: "a_cx", harness: "codex", live: true }, { handle: "a_cl", harness: "claude", live: true }];
    function render(more: Record<string, unknown>) {
      const root = document.createElement("div");
      document.body.appendChild(root);
      const props = { versions: [], shown: 1, agent: "claude", threads, resolved: {}, now: new Date(base.created_at), selected: null,
        onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn(), selection: { ids: [], anchor: null }, onToggle: vi.fn(), onChoose: vi.fn(), ...more };
      const view = mount(Sidebar, props, root);
      return { root, props, done: () => { view.unmount(); root.remove(); } };
    }
    const box = (root: HTMLElement, id: string) => root.querySelector<HTMLInputElement>(`.thread-card[data-thread="${id}"] .thread-check`)!;

    it("gives open cards a labelled box, ticked from the selection, and none to resolved cards", () => {
      const { root, done } = render({ selection: { ids: ["a"], anchor: "a" } });
      expect(box(root, "a").getAttribute("aria-label")).toBe("Select thread 1 «Goals»");
      expect(box(root, "a").checked).toBe(true);
      expect(box(root, "b").checked).toBe(false);
      expect(root.querySelector('.thread-card[data-thread="c"] .thread-check')).toBeNull();
      done();
    });

    it("passes shift on a Shift-click, for a range", () => {
      const { root, props, done } = render({});
      flush(() => box(root, "b").dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true })));
      expect(props.onToggle).toHaveBeenCalledWith(expect.objectContaining({ id: "b" }), true, ["a", "b"]);
      flush(() => box(root, "a").click());
      expect(props.onToggle).toHaveBeenLastCalledWith(expect.objectContaining({ id: "a" }), false, ["a", "b"]);
      done();
    });

    it("with two live agents, the caret's menu lists both and checks the target", () => {
      const { root, props, done } = render({ agents: two, sendTo: "a_cl" });
      const card = root.querySelector('.thread-card[data-thread="a"]')!;
      expect(card.querySelector(".send .primary")!.textContent).toBe("Send to claude");
      flush(() => card.querySelector<HTMLButtonElement>('[aria-label="Choose the agent"]')!.click());
      const items = Array.from(card.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]'));
      expect(items.map(i => [i.textContent, i.getAttribute("aria-checked")])).toEqual([["codex", "false"], ["claude", "true"]]);
      flush(() => items[0].click());
      expect(props.onChoose).toHaveBeenCalledWith("a_cx");
      expect(card.querySelector('[role="menu"]')).toBeNull();
      done();
    });

    it("with one live agent, there is no caret", () => {
      const { root, done } = render({ agents: [two[1], { handle: "a_old", harness: "pi", live: false }], sendTo: "a_cl" });
      expect(root.querySelector('[aria-label="Choose the agent"]')).toBeNull();
      done();
    });

    it("offers Send N unsent at the top, to the target", () => {
      const onSendUnsent = vi.fn();
      const { root, done } = render({ agents: two, sendTo: "a_cx", onSendUnsent });
      const b = root.querySelector<HTMLButtonElement>(".send-unsent")!;
      expect(b.textContent).toBe("Send 2 unsent to codex");
      flush(() => b.click());
      expect(onSendUnsent).toHaveBeenCalledOnce();
      done();
    });
  });
});


describe("Sidebar: threads on other pages", () => {
  const onAbout = { ...anchor, file: "about.html" };
  const here: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "here")] };
  // Which threads are open in place outlives a sidebar (for the page's life): each test has threads of its own.
  let made = 0;
  const thereOf = (id: string): Thread => ({ ...base, id, anchor: onAbout, status: "open", sent_to_agent: false, has_clip: true, clip_url: `/api/artifacts/7q3k9mzx2b4t/threads/${id}/clip`,
    comments: [comment("2", "viewer", "Alex", "Button looks off"), comment("3", "agent", "claude", "Fixed in v4"), comment("4", "viewer", "Alex", "Still misaligned")] });
  const doneOf = (id: string): Thread => ({ ...base, id, anchor: onAbout, status: "resolved", sent_to_agent: false, comments: [comment("5", "viewer", "Alex", "done there")] });
  function setup() {
    made++;
    const b = `b${made}`, c = `c${made}`;
    const there = thereOf(b), done = doneOf(c);
    const threads = [here, there, done];
    const calls = { select: vi.fn(), send: vi.fn(), resolve: vi.fn(), reply: vi.fn() };
    const props = { versions: [], shown: 1, agent: "claude", threads, resolved: {}, file: "index.html", now: new Date(base.created_at), selected: null,
      onSelect: calls.select, onSend: calls.send, onResolve: calls.resolve, onReply: calls.reply };
    const view = mount(Sidebar, props);
    const card = (id: string) => view.root.querySelector<HTMLElement>(`[data-thread="${id}"]`)!;
    const head = (id: string) => card(id).querySelector<HTMLButtonElement>("button.card-head")!;
    return { view, props, calls, card, head, b, c, there, done };
  }

  it("folds another page's thread to its summary, and opens it in place without selecting it", () => {
    const { view, calls, card, head, b, there } = setup();
    // This page's thread is whole, as before.
    expect(head("a").hasAttribute("aria-expanded")).toBe(false);
    expect(card(b).classList.contains("folded")).toBe(true);
    expect(head(b).getAttribute("aria-expanded")).toBe("false");
    expect(head(b).getAttribute("aria-controls")).toBe(`thread-${b}-body`);
    expect(card(b).querySelector(`#thread-${b}-body`)!.textContent).toContain("Button looks off");
    expect(card(b).querySelector(".fold-body")!.textContent).toBe("Button looks off");
    expect(card(b).querySelector(".fold-meta")!.textContent).toBe("Alex2 replies");
    expect(card(b).querySelector('input[aria-label="Reply"]')).toBeNull();
    // A click anywhere on the folded card opens it; neither selects it.
    card(b).querySelector<HTMLElement>(".fold-body")!.click();
    flush();
    expect(head(b).getAttribute("aria-expanded")).toBe("true");
    expect(Array.from(card(b).querySelectorAll(".msg .body"), x => x.textContent)).toEqual(["Button looks off", "Fixed in v4", "Still misaligned"]);
    head(b).click();
    flush();
    expect(head(b).getAttribute("aria-expanded")).toBe("false");
    expect(calls.select).not.toHaveBeenCalled();
    // "Go to page" opens its page (the select that navigates).
    card(b).querySelector<HTMLButtonElement>("button.go-page")!.click();
    expect(calls.select).toHaveBeenCalledWith(there);
    expect(card(b).querySelector("button.go-page")!.getAttribute("aria-label")).toBe("Go to page about.html");
    view.unmount();
  });

  it("replies to, resolves, sends and reopens another page's thread from its open card", () => {
    const { view, calls, card, head, b, c, there, done } = setup();
    head(b).click();
    flush();
    const input = card(b).querySelector<HTMLInputElement>('input[aria-label="Reply"]')!;
    input.value = "@claude still off by 2px";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    flush();
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(calls.reply).toHaveBeenCalledWith(there, "@claude still off by 2px");
    const button = (id: string, name: string) => Array.from(card(id).querySelectorAll<HTMLButtonElement>(".actions button")).find(x => x.textContent === name)!;
    button(b, "Send to claude").click();
    expect(calls.send).toHaveBeenCalledWith(there);
    button(b, "Resolve").click();
    expect(calls.resolve).toHaveBeenCalledWith(there);
    // The tools follow the reply box, as the card's actions do.
    expect(Array.from(card(b).querySelectorAll(".actions button"), x => x.textContent)).toEqual(["Resolve", "Send to claude", "Go to page ↗"]);
    (view.root.querySelector<HTMLDetailsElement>(".section-resolved")!).open = true;
    head(c).click();
    flush();
    button(c, "Reopen").click();
    expect(calls.resolve).toHaveBeenCalledWith(done);
    expect(calls.select).not.toHaveBeenCalled();
    view.unmount();
  });

  it("keeps cards open by thread ID through the list's changes, shows new replies, and folds with Escape, focus on its head", () => {
    const { view, props, card, head, b, c, there, done } = setup();
    head(b).click();
    flush();
    (view.root.querySelector<HTMLDetailsElement>(".section-resolved")!).open = true;
    head(c).click();
    flush();
    const more = { ...there, comments: [...there.comments, comment("6", "agent", "claude", "Aligned now")] };
    view.update({ ...props, threads: [here, more, done] });
    expect(head(b).getAttribute("aria-expanded")).toBe("true");
    expect(head(c).getAttribute("aria-expanded")).toBe("true");
    expect(card(b).textContent).toContain("Aligned now");
    const input = card(b).querySelector<HTMLInputElement>('input[aria-label="Reply"]')!;
    input.focus();
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    flush();
    expect(head(b).getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(head(b));
    expect(head(c).getAttribute("aria-expanded")).toBe("true");
    view.unmount();
  });

  it("shows the clip of an open card, which enlarges and closes with Escape or a click outside", () => {
    const { view, card, head, b, there } = setup();
    expect(card(b).querySelector(".clip-thumb")).toBeNull();
    head(b).click();
    flush();
    const thumb = card(b).querySelector<HTMLButtonElement>(".clip-thumb")!;
    expect(thumb.getAttribute("aria-label")).toBe("Enlarge the screenshot");
    expect(thumb.querySelector("img")!.getAttribute("src")).toBe(there.clip_url);
    expect(thumb.querySelector("img")!.getAttribute("loading")).toBe("lazy");
    const dialog = card(b).querySelector("dialog")!;
    thumb.click();
    expect(dialog.open).toBe(true);
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    flush();
    expect(dialog.open).toBe(false);
    expect(head(b).getAttribute("aria-expanded")).toBe("true");
    thumb.click();
    dialog.click();
    expect(dialog.open).toBe(false);
    view.unmount();
  });

  it("marks a folded card's thread looked only once it is open and seen", () => {
    vi.useFakeTimers();
    const seen = new Map<Element, (e: { intersectionRatio: number }[]) => void>();
    vi.stubGlobal("IntersectionObserver", class { constructor(private cb: (e: { intersectionRatio: number }[]) => void) {} observe(el: Element) { seen.set(el, this.cb); } disconnect() {} });
    try {
      const onSeen = vi.fn();
      made++;
      const there = thereOf(`b${made}`);
      const view = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads: [here, there], resolved: {}, file: "index.html", now: new Date(base.created_at), selected: null,
        onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn(), onSeen });
      const card = () => view.root.querySelector(`[data-thread="${there.id}"]`)!;
      // Folded, it is not watched at all; this page's card is.
      expect(seen.has(card())).toBe(false);
      expect(seen.has(view.root.querySelector(`[data-thread="${here.id}"]`)!)).toBe(true);
      vi.advanceTimersByTime(2000);
      expect(onSeen).not.toHaveBeenCalledWith(there);
      card().querySelector<HTMLButtonElement>(".card-head")!.click();
      flush();
      seen.get(card())!([{ intersectionRatio: 1 }]);
      vi.advanceTimersByTime(1000);
      expect(onSeen).toHaveBeenCalledWith(there);
      view.unmount();
    } finally {
      vi.unstubAllGlobals();
      vi.useRealTimers();
    }
  });

  it("keeps a card open when the sidebar closes and opens again", () => {
    const first = setup();
    first.head(first.b).click();
    flush();
    first.view.unmount();
    const again = mount(Sidebar, first.props);
    expect(again.root.querySelector(`[data-thread="${first.b}"] .card-head`)!.getAttribute("aria-expanded")).toBe("true");
    again.unmount();
  });

  it("moves focus with a card reopened from inside it to its head in its new group", async () => {
    const { view, props, head, card, c, done } = setup();
    (view.root.querySelector<HTMLDetailsElement>(".section-resolved")!).open = true;
    head(c).click();
    flush();
    const reopen = Array.from(card(c).querySelectorAll<HTMLButtonElement>(".actions button")).find(x => x.textContent === "Reopen")!;
    reopen.focus();
    reopen.click();
    view.update({ ...props, threads: [here, props.threads[1], { ...done, status: "open" }] });
    await vi.waitFor(() => expect(document.activeElement).toBe(head(c)));
    expect(card(c).closest(".section-open")).not.toBeNull();
    expect(document.activeElement).toBe(head(c));
    view.unmount();
  });

  it("takes no focus for a card whose resolve failed when its status changes later", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      const { view, props, head, card, c, done } = setup();
      (view.root.querySelector<HTMLDetailsElement>(".section-resolved")!).open = true;
      head(c).click();
      flush();
      const reopen = Array.from(card(c).querySelectorAll<HTMLButtonElement>(".actions button")).find(x => x.textContent === "Reopen")!;
      reopen.focus();
      reopen.click();
      // The reopen failed: nothing changed. Someone else reopens it a minute later, while the viewer types elsewhere.
      vi.setSystemTime(Date.now() + 60_000);
      const elsewhere = document.createElement("input");
      document.body.append(elsewhere);
      elsewhere.focus();
      view.update({ ...props, threads: [here, props.threads[1], { ...done, status: "open" }] });
      await Promise.resolve();
      flush();
      expect(document.activeElement).toBe(elsewhere);
      elsewhere.remove();
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  });
});
