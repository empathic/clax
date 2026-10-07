import { cleanup, fireEvent, render, screen, within } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PanelState, PanelToWorker, SiteView } from "../messages";
import Panel from "./Panel.svelte";

const thread = {
  id: "01J9AAAAAAAAAAAAAAAAAAAAAA", artifact_id: "7q3k9mzx2b4t", version_n: 1, status: "open", sent_to_agent: false, has_clip: false,
  anchor: { kind: "element", selector: "#save", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
  comments: [{ id: "c1", thread_id: "01J9AAAAAAAAAAAAAAAAAAAAAA", author_kind: "viewer", author_name: "Alex", via_harness: null, body: "Too wide", created_at: "2026-10-05T10:00:00.000Z" }],
  created_at: "2026-10-05T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null, addressed_in: [],
};
function state(over: Partial<PanelState> = {}): PanelState {
  return {
    tabId: 3, url: "http://localhost:5173/", route: null, resolved: { [thread.id]: { id: thread.id, found: true, method: "exact", rect: null } },
    page: { artifact_id: "7q3k9mzx2b4t", origin: "http://localhost:5173", path: "/", page_url: "http://localhost:5173/", title: "Home", current_version: 1, url: "http://localhost:7480/a/7q3k9mzx2b4t" },
    threads: [thread as never], versions: [{ artifact_id: "7q3k9mzx2b4t", n: 1, label: null, created_at: "2026-10-05T10:00:00.000Z", files: {} }],
    working: [], participants: { people: [], agents: [{ handle: `a_${"1".repeat(22)}`, harness: "claude", live: true }] },
    viewer: { public_id: "u_x", display_name: "Alex" }, commentMode: false, enabled: true, selected: null, error: null, ...over,
  };
}
function link(s: PanelState, up?: boolean) {
  const sent: PanelToWorker[] = [];
  return { sent, state: s, up, post: (m: PanelToWorker) => sent.push(m) };
}
afterEach(() => cleanup());

describe("Panel", () => {
  it("lists the page's threads and sends one to the live agent", async () => {
    const l = link(state());
    render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.getByText("Too wide")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: /Send to claude/ }), { detail: 1 });
    expect(l.sent).toContainEqual({ t: "send", threadId: thread.id, to: `a_${"1".repeat(22)}` });
  });

  it("asks for the owner's name while the owner has none", async () => {
    const l = link(state({ viewer: { public_id: "u_x", display_name: null } }));
    render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    const input = screen.getByLabelText("Your name") as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "Mia" } });
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(l.sent).toContainEqual({ t: "set-name", name: "Mia" });
  });

  it("shows a failure with a way to retry", async () => {
    const l = link(state({ error: { code: "host_missing", message: "Specified native messaging host not found." } }));
    render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.getByText(/clax init/)).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(l.sent).toContainEqual({ t: "retry" });
  });

  it("shows an action's failure again after a dismissal when the action fails again", async () => {
    const error = { code: "no_capture_permission", message: "Clax needs a click on its button to take screenshots on this tab." };
    const l = { ...link(state({ error })), failures: 1 };
    const { rerender } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z"), shortcut: Promise.resolve("⌥⇧C") } });
    // In a tab Clax is on, its button turns Clax off: the panel names the command's shortcut instead.
    await vi.waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Press ⌥⇧C on the page to comment with a screenshot."));
    expect(screen.getByRole("alert").textContent).not.toContain("Clax button");
    await fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("alert")).toBeNull();
    // The worker pushes the tab's state again: the dismissed failure stays hidden.
    await rerender({ link: { ...l, state: state({ error }) } as never });
    expect(screen.queryByRole("alert")).toBeNull();
    // The person presses Comment again and it fails the same way: told again.
    await rerender({ link: { ...l, state: state({ error }), failures: 2 } as never });
    expect(screen.getByRole("alert").textContent).toContain("Press ⌥⇧C");
  });

  it("names the page's context menu when the command has no shortcut", async () => {
    const error = { code: "no_capture_permission", message: "m" };
    render(Panel, { props: { link: link(state({ error })) as never, now: new Date("2026-10-05T10:01:00.000Z"), shortcut: Promise.resolve(null) } });
    await vi.waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Right-click the page and choose Comment with Clax to comment with a screenshot."));
  });

  it("offers to start when Clax is off on the tab, naming the command's shortcut or else the context menu", async () => {
    const off = state({ enabled: false, page: null, threads: [] });
    render(Panel, { props: { link: link(off) as never, shortcut: Promise.resolve("⌥⇧C") } });
    await vi.waitFor(() => expect(screen.getByText("Click the Clax button or press ⌥⇧C on a page to comment on it.")).toBeTruthy());
    cleanup();
    render(Panel, { props: { link: link(off) as never, shortcut: Promise.resolve(null) } });
    expect(screen.getByText("Click the Clax button, or right-click a page and choose Comment with Clax, to comment on it.")).toBeTruthy();
  });

  it("says when the worker's stream is down, so what it shows may be stale", () => {
    render(Panel, { props: { link: link(state(), false) as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.getByText(/out of date/).getAttribute("role")).toBe("status");
  });

  it("shows the page's title and comments as text, never as markup", () => {
    const hostile = { ...thread, comments: [{ ...thread.comments[0], body: "<img src=x onerror=alert(1)>" }] };
    const l = link(state({ threads: [hostile as never], page: { ...state().page!, title: "<b>Home</b>" } }));
    const { container } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.getByText("<b>Home</b>")).toBeTruthy();
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeTruthy();
    expect(container.querySelector("h1 b, img[src=x]")).toBeNull();
  });

  it("opens a card on another route in place, and goes to its route only from Go to page", async () => {
    const other = { ...thread, anchor: { ...thread.anchor, route: "?tab=billing" } };
    const l = link(state({ threads: [other as never], resolved: {} }));
    const { container } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(container.textContent).toContain("?tab=billing");
    const head = container.querySelector<HTMLButtonElement>(".card-head")!;
    expect(head.getAttribute("aria-expanded")).toBe("false");
    await fireEvent.click(head);
    expect(head.getAttribute("aria-expanded")).toBe("true");
    await fireEvent.input(screen.getByLabelText("Reply"), { target: { value: "Here too" } });
    await fireEvent.keyDown(screen.getByLabelText("Reply"), { key: "Enter" });
    expect(l.sent).toEqual([{ t: "reply", threadId: thread.id, body: "Here too" }]);
    await fireEvent.click(screen.getByRole("button", { name: "Go to page ?tab=billing" }));
    expect(l.sent.slice(1)).toEqual([{ t: "navigate", route: "?tab=billing", artifactId: "7q3k9mzx2b4t" }, { t: "select", threadId: thread.id }]);
  });

  it("shows who is here on the page beside its agents", () => {
    const l = link(state({ presence: [{ public_id: "u_y", display_name: "Mia Wong", state: "here", where: null, since: "2026-10-05T10:00:00.000Z" }] }));
    const { container } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(container.querySelector(".tok.p.here")?.textContent).toBe("MW");
    expect(container.querySelector(".tok.a")?.textContent).toBe("cl");
  });

  it("offers Retry only for a failure trying again can fix, and lets others be dismissed", async () => {
    const l = link(state({ error: { code: "not_found", message: "No such thread." } }));
    render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.getByText("No such thread.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    await fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("No such thread.")).toBeNull();
    expect(l.sent).toEqual([]);
  });

  it("words an unknown failure code by its message, never by a property of the help table", () => {
    render(Panel, { props: { link: link(state({ error: { code: "constructor", message: "Something broke." } })) as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.getByText("Something broke.")).toBeTruthy();
  });

  it("shows the setup command as code with a button that copies it", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { container } = render(Panel, { props: { link: link(state({ error: { code: "host_missing", message: "x" } })) as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(container.querySelector(".notice code")?.textContent).toBe("clax init");
    expect(container.querySelector(".notice")?.textContent).not.toContain("`");
    await fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("clax init");
  });

  it("sends the same name again after a failed save", async () => {
    const l = link(state({ viewer: { public_id: "u_x", display_name: null } }));
    const view = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    const input = screen.getByLabelText("Your name") as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "Mia" } });
    await fireEvent.keyDown(input, { key: "Enter" });
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(l.sent.filter(m => m.t === "set-name")).toHaveLength(1);
    await view.rerender({ link: { ...l, state: { ...l.state, error: { code: "invalid_name", message: "Not a name." } } } as never, now: new Date("2026-10-05T10:01:00.000Z") });
    await fireEvent.keyDown(screen.getByLabelText("Your name"), { key: "Enter" });
    expect(l.sent.filter(m => m.t === "set-name")).toHaveLength(2);
  });

  it("turns Clax off in its tab, with or without a live page, and offers it only where Clax is on", async () => {
    const l = link(state());
    const view = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    await fireEvent.click(screen.getByRole("button", { name: "Turn off in this tab" }));
    expect(l.sent).toContainEqual({ t: "turn-off", tabId: 3 });
    await view.rerender({ link: link(state({ page: null, threads: [] })) as never });
    expect(screen.getByRole("button", { name: "Turn off in this tab" })).toBeTruthy();
    await view.rerender({ link: link(state({ enabled: false, page: null, threads: [] })) as never });
    expect(screen.queryByRole("button", { name: "Turn off in this tab" })).toBeNull();
  });
});

describe("Panel: the site's other pages", () => {
  const O = "http://localhost:5173";
  const far = (id: string, aid: string, path: string, body: string, at: string, extra: object = {}) => ({
    ...thread, id, artifact_id: aid, page_path: path, page_url: O + path, created_at: at,
    comments: [{ ...thread.comments[0], id: `c${id}`, thread_id: id, body, created_at: at }], ...extra,
  });
  const fpage = (aid: string, path: string, extra: object = {}) => ({ artifact_id: aid, origin: O, path, page_url: O + path, title: path, current_version: 1, url: `http://localhost:7480/a/${aid}`, ...extra });
  const T2 = "01J9BBBBBBBBBBBBBBBBBBBBBB", T3 = "01J9CCCCCCCCCCCCCCCCCCCCCC", T4 = "01J9DDDDDDDDDDDDDDDDDDDDDD";
  const RULE = "01J9EEEEEEEEEEEEEEEEEEEEEE";
  const site = (): SiteView => ({ origin: O, rules: [{ id: RULE, origin: O, pattern: "/users/:id", page_url: `${O}/users/:id`, created_at: "t" }], pages: [
    { page: state().page!, threads: [thread as never] },
    { page: fpage("8r4m0nzy3c5v", "/billing/a-very-long-path-that-goes-on-and-on/and-on"), threads: [far(T2, "8r4m0nzy3c5v", "/billing/a-very-long-path-that-goes-on-and-on/and-on", "Old <img src=x onerror=alert(1)>", "2026-10-01T10:00:00.000Z", { status: "resolved" }) as never] },
    { page: fpage("9s5n1pza4d6w", "/users/:id", { merged: true, pattern: "/users/:id" }), threads: [
      far(T3, "9s5n1pza4d6w", "/users/2", "Avatar is blurry", "2026-10-05T09:00:00.000Z") as never,
      far(T4, "9s5n1pza4d6w", "/users/3", "Name wraps", "2026-10-05T09:30:00.000Z", { addressed_in: [2] }) as never,
    ] },
    { page: fpage("aaaaaaaaaaaa", "/users/9"), threads: [] },
  ] });
  function area() {
    const data: Record<string, unknown> = {};
    return { data, get: async (k: string) => (k in data ? { [k]: structuredClone(data[k]) } : {}), set: async (v: Record<string, unknown>) => { Object.assign(data, structuredClone(v)); } };
  }
  function siteLink(steps: { moved: number; remaining: number }[] = []) {
    const l = { ...link(state()), site: site(), asked: [] as unknown[],
      request: async (m: unknown) => { l.asked.push(m); const r = steps.shift(); if (!r) throw new Error("Not a pattern Clax can merge."); return r; } };
    return l;
  }
  const NOW = new Date("2026-10-05T10:01:00.000Z");
  const settle = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };

  it("lists the other pages' threads under each page, newest activity first, with their counts, as text", () => {
    const { container } = render(Panel, { props: { link: siteLink() as never, now: NOW, store: area() } });
    expect(screen.getByText("This page")).toBeTruthy();
    const paths = [...container.querySelectorAll(".elsewhere summary .path")].map(e => e.textContent);
    expect(paths).toEqual(["/users/:id", "/billing/a-very-long-path-that-goes-on-and-on/and-on"]);
    expect(container.querySelector(".elsewhere summary .path")!.getAttribute("title")).toBe("/users/:id");
    expect([...container.querySelectorAll(".elsewhere details")[0].querySelectorAll(".counts .n")].map(e => e.textContent)).toEqual(["1 open", "1 addressed"]);
    expect([...container.querySelectorAll(".far .fold-body")].map(e => e.textContent)).toEqual(["Name wraps", "Avatar is blurry", "Old <img src=x onerror=alert(1)>"]);
    expect(container.querySelector(".far img")).toBeNull();
    expect(screen.getByText("at /users/2")).toBeTruthy();
    // A page with no threads is not listed.
    expect(container.textContent).not.toContain("/users/9");
  });

  it("opens a thread of another page in place without moving the tab, opens its page from Go to page, and says when its pin is on this screen", async () => {
    const l = siteLink();
    const { container } = render(Panel, { props: { link: { ...l, state: { ...state(), resolved: { ...state().resolved, [T3]: { id: T3, found: true, method: "selector", rect: null } } } } as never, now: NOW, store: area() } });
    expect(screen.getAllByText("Pinned here")).toHaveLength(1);
    await fireEvent.click(screen.getByText("Avatar is blurry"));
    const card = container.querySelector<HTMLElement>(`[data-thread="${T3}"]`)!;
    expect(card.querySelector(".card-head")!.getAttribute("aria-expanded")).toBe("true");
    expect(within(card).getByLabelText("Reply")).toBeTruthy();
    // Nothing goes to the worker: the tab stays, nothing is selected or pinned.
    const acts = () => l.sent.filter(m => m.t !== "suggest");
    expect(acts()).toEqual([]);
    await fireEvent.click(within(card).getByRole("button", { name: "Go to page /users/2" }));
    await settle();
    expect(acts()).toEqual([{ t: "open-thread", threadId: T3 }]);
  });

  it("replies to, resolves, reopens and sends another page's thread from its card, @agent included, to its page's agent as there", async () => {
    const A1 = `a_${"1".repeat(22)}`, A2 = `a_${"2".repeat(22)}`;
    const asked: string[] = [];
    const l = { ...siteLink(), farPage: async (id: string) => {
      asked.push(id);
      return { artifactId: "9s5n1pza4d6w", agents: [{ handle: A1, harness: "claude", live: true }, { handle: A2, harness: "codex", live: true }],
        versions: [{ artifact_id: "9s5n1pza4d6w", n: 1, label: null, created_at: "2026-10-05T08:00:00.000Z", files: {} }, { artifact_id: "9s5n1pza4d6w", n: 2, label: null, created_at: "2026-10-05T09:10:00.000Z", files: {} }] };
    } };
    const { container } = render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    const head = () => container.querySelector<HTMLButtonElement>(`[data-thread="${T3}"] .card-head`)!;
    await fireEvent.click(head());
    await settle();
    expect(asked).toEqual([T3]);
    const card = container.querySelector<HTMLElement>(`[data-thread="${T3}"]`)!;
    // Each comment with its author, version and time; no history repeating them.
    expect(card.querySelector(".msg .by")!.textContent).toBe("Alexv11 h ago");
    expect(card.querySelector(".hist")).toBeNull();
    await fireEvent.input(within(card).getByLabelText("Reply"), { target: { value: "@agent the avatar still blurs" } });
    await fireEvent.click(within(card).getByRole("button", { name: "Reply" }), { detail: 1 });
    // Send names the page's first live agent, and its picker offers the others, as on that page.
    await fireEvent.click(within(card).getByRole("button", { name: "Send to claude" }), { detail: 1 });
    await fireEvent.click(within(card).getByRole("button", { name: "Choose the agent" }));
    await fireEvent.click(within(card).getByRole("menuitemradio", { name: "codex" }));
    await fireEvent.click(within(card).getByRole("button", { name: "Send to codex" }), { detail: 1 });
    await fireEvent.click(within(card).getByRole("button", { name: "Resolve" }), { detail: 1 });
    expect(l.sent.filter(m => m.t !== "suggest")).toEqual([
      { t: "reply", threadId: T3, body: "@agent the avatar still blurs" },
      { t: "send", threadId: T3, to: A1 },
      { t: "send", threadId: T3, to: A2 },
      { t: "resolve", threadId: T3 },
    ]);
    // A resolved thread of another page reopens from its card.
    const old = container.querySelector<HTMLElement>(`[data-thread="${T2}"]`)!;
    await fireEvent.click(old.querySelector(".card-head")!);
    await fireEvent.click(within(old).getByRole("button", { name: "Reopen" }), { detail: 1 });
    expect(l.sent.at(-1)).toEqual({ t: "reopen", threadId: T2 });
  });

  it("marks another page's thread looked once its open card is seen, never while folded", async () => {
    vi.useFakeTimers();
    const seen = new Map<Element, (e: { intersectionRatio: number }[]) => void>();
    vi.stubGlobal("IntersectionObserver", class { constructor(private cb: (e: { intersectionRatio: number }[]) => void) {} observe(el: Element) { seen.set(el, this.cb); } disconnect() {} });
    try {
      const l = siteLink();
      const { container } = render(Panel, { props: { link: l as never, now: NOW, store: area() } });
      const card = () => container.querySelector(`[data-thread="${T3}"]`)!;
      expect(seen.has(card())).toBe(false);
      await fireEvent.click(card().querySelector(".card-head")!);
      seen.get(card())!([{ intersectionRatio: 1 }]);
      await vi.advanceTimersByTimeAsync(1000);
      expect(l.sent).toContainEqual({ t: "looked", threadIds: [T3] });
    } finally {
      vi.unstubAllGlobals();
      vi.useRealTimers();
    }
  });

  it("tags another page's events only once its versions are known, reads its agents again when stale, and says when they could not be read, with Retry", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      const A1 = `a_${"1".repeat(22)}`;
      const answers: (object | null)[] = [null, { artifactId: "9s5n1pza4d6w", agents: [{ handle: A1, harness: "claude", live: true }],
        versions: [{ n: 1, created_at: "2026-10-05T08:00:00.000Z", agent_harness: null }, { n: 5, created_at: "2026-10-05T09:40:00.000Z", agent_harness: null }] }];
      let asked = 0;
      let release: () => void = () => {};
      const l = { ...siteLink(), farPage: async () => { asked++; const a = answers[Math.min(asked - 1, answers.length - 1)]; await new Promise<void>(r => { release = r; }); return a; } };
      const s = site();
      const p = s.pages[2];
      s.pages[2] = { ...p, threads: [{ ...(p.threads[0] as object), sends: [{ batch_id: "b", size: 1, note: null, sent_by: "Alex", sent_at: "2026-10-05T09:50:00.000Z" }] } as never, p.threads[1]] };
      const { container } = render(Panel, { props: { link: { ...l, site: s } as never, now: NOW, store: area() } });
      const card = () => container.querySelector<HTMLElement>(`[data-thread="${T3}"]`)!;
      await fireEvent.click(card().querySelector(".card-head")!);
      // Before the page's versions: the send has no tag (never a guessed v1).
      expect(card().querySelector(".hist")!.textContent).toBe("Alex sent it");
      expect(card().querySelectorAll(".msg .by .vt")).toHaveLength(1);
      release();
      await settle();
      // The read failed: Send says why, and Retry reads again.
      const send = within(card()).getByRole("button", { name: "Send to agent" }) as HTMLButtonElement;
      expect(send.disabled).toBe(true);
      expect(card().querySelector(".far-err")!.textContent).toContain("Could not load this page's agents.");
      expect(card().querySelector(".hist")!.textContent).toBe("Alex sent it");
      await fireEvent.click(within(card()).getByRole("button", { name: "Retry" }));
      release();
      await settle();
      expect(asked).toBe(2);
      expect(card().querySelector(".far-err")).toBeNull();
      expect(card().querySelector(".hist")!.textContent).toBe("v5 Alex sent it");
      expect((within(card()).getByRole("button", { name: "Send to claude" }) as HTMLButtonElement).disabled).toBe(false);
      // Hovered soon after, the agents are not read again; 30 s later they are.
      await fireEvent.pointerEnter(card().parentElement!);
      expect(asked).toBe(2);
      vi.setSystemTime(Date.now() + 31_000);
      await fireEvent.pointerEnter(card().parentElement!);
      expect(asked).toBe(3);
      release();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps a card open while a search hides the list, and offers no Send before its page's agents are known", async () => {
    const l = siteLink();
    const { container } = render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    await fireEvent.click(container.querySelector(`[data-thread="${T3}"] .card-head`)!);
    expect((within(container.querySelector<HTMLElement>(`[data-thread="${T3}"]`)!).getByRole("button", { name: "Send to agent" }) as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(screen.getByLabelText("Search comments"), { target: { value: "nothing matches this" } });
    expect(container.querySelector(`[data-thread="${T3}"]`)).toBeNull();
    await fireEvent.input(screen.getByLabelText("Search comments"), { target: { value: "" } });
    expect(container.querySelector(`[data-thread="${T3}"] .card-head`)!.getAttribute("aria-expanded")).toBe("true");
  });

  it("keeps several cards open through the listing's changes, shows replies as they come, and folds one with Escape", async () => {
    const l = siteLink();
    const view = render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    const card = (id: string) => view.container.querySelector<HTMLElement>(`[data-thread="${id}"]`)!;
    await fireEvent.click(card(T3).querySelector(".card-head")!);
    await fireEvent.click(card(T4).querySelector(".card-head")!);
    const next = site();
    const p = next.pages[2];
    const t3 = p.threads[0] as unknown as { comments: object[] };
    next.pages[2] = { ...p, threads: [{ ...t3, comments: [...t3.comments, { ...thread.comments[0], id: "c9", thread_id: T3, author_kind: "agent", author_name: "claude", via_harness: "claude", body: "Sharper now", created_at: "2026-10-05T10:00:30.000Z" }] } as never, p.threads[1]] };
    await view.rerender({ link: { ...l, site: next } as never, now: NOW, store: area() });
    expect(within(card(T3)).getByText("Sharper now")).toBeTruthy();
    expect(card(T4).querySelector(".card-head")!.getAttribute("aria-expanded")).toBe("true");
    const input = within(card(T3)).getByLabelText("Reply");
    input.focus();
    await fireEvent.keyDown(input, { key: "Escape" });
    const head = card(T3).querySelector<HTMLButtonElement>(".card-head")!;
    expect(head.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(head);
    expect(card(T4).querySelector(".card-head")!.getAttribute("aria-expanded")).toBe("true");
  });

  it("shows another page's clip through the worker once its card opens, and enlarges it until Escape", async () => {
    const l = { ...siteLink(), clips: [] as string[], clip: async (id: string) => { l.clips.push(id); return "data:image/png;base64,iVBORw0KGgo="; } };
    const s = site();
    const p = s.pages[2];
    s.pages[2] = { ...p, threads: [{ ...(p.threads[0] as object), has_clip: true, clip_url: `/api/artifacts/9s5n1pza4d6w/threads/${T3}/clip` } as never, p.threads[1]] };
    const { container } = render(Panel, { props: { link: { ...l, site: s } as never, now: NOW, store: area() } });
    expect(l.clips).toEqual([]);
    const card = container.querySelector<HTMLElement>(`[data-thread="${T3}"]`)!;
    await fireEvent.click(card.querySelector(".card-head")!);
    await settle();
    expect(l.clips).toEqual([T3]);
    const thumb = within(card).getByRole("button", { name: "Enlarge the screenshot" });
    expect(thumb.querySelector("img")!.getAttribute("src")).toBe("data:image/png;base64,iVBORw0KGgo=");
    await fireEvent.click(thumb);
    const dialog = card.querySelector("dialog")!;
    expect(dialog.open).toBe(true);
    await fireEvent.keyDown(dialog, { key: "Escape" });
    expect(dialog.open).toBe(false);
    // Escape closed only the enlarged clip: the card stays open.
    expect(card.querySelector(".card-head")!.getAttribute("aria-expanded")).toBe("true");
    await fireEvent.click(thumb);
    await fireEvent.click(dialog);
    expect(dialog.open).toBe(false);
  });

  it("filters both lists by status and searches them, remembering the filter for the site", async () => {
    const store = area();
    const { container } = render(Panel, { props: { link: siteLink() as never, now: NOW, store } });
    await fireEvent.click(screen.getByRole("button", { name: "Resolved" }));
    expect([...container.querySelectorAll(".far .fold-body")].map(e => e.textContent)).toEqual(["Old <img src=x onerror=alert(1)>"]);
    expect(screen.queryByText("Too wide")).toBeNull();
    expect(screen.getByText("Nothing on this page matches.")).toBeTruthy();
    await settle();
    expect(store.data[`site-prefs:${O}`]).toEqual({ filter: "resolved", collapsed: [] });
    await fireEvent.click(screen.getByRole("button", { name: "All" }));
    await fireEvent.input(screen.getByLabelText("Search comments"), { target: { value: "users/3" } });
    expect([...container.querySelectorAll(".far .fold-body")].map(e => e.textContent)).toEqual(["Name wraps"]);
    expect(screen.queryByText("Too wide")).toBeNull();
    // A new panel for the site starts with what was remembered.
    cleanup();
    render(Panel, { props: { link: siteLink() as never, now: NOW, store } });
    await settle();
    expect(screen.getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("keeps a group the person collapsed collapsed, per site", async () => {
    const store = area();
    const { container } = render(Panel, { props: { link: siteLink() as never, now: NOW, store } });
    const group = container.querySelector(".elsewhere details") as HTMLDetailsElement;
    expect(group.open).toBe(true);
    group.open = false;
    await fireEvent(group, new Event("toggle"));
    await settle();
    expect(store.data[`site-prefs:${O}`]).toEqual({ filter: "all", collapsed: ["/users/:id"] });
    cleanup();
    const again = render(Panel, { props: { link: siteLink() as never, now: NOW, store } });
    await settle();
    expect((again.container.querySelector(".elsewhere details") as HTMLDetailsElement).open).toBe(false);
  });

  it("moves a thread of another page here, and the page's selected thread to another page", async () => {
    const l = siteLink();
    render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    await fireEvent.click(screen.getAllByRole("button", { name: "Move…" })[0]);
    const pick = screen.getByLabelText("Move to page") as HTMLSelectElement;
    expect([...pick.options].map(o => o.textContent)).toEqual(["This page (/)", "/billing/a-very-long-path-that-goes-on-and-on/and-on", "/users/9"]);
    await fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(l.sent).toContainEqual({ t: "move", threadId: T4, pageUrl: "http://localhost:5173/" });
    cleanup();
    const m = siteLink();
    render(Panel, { props: { link: { ...m, state: state({ selected: thread.id }) } as never, now: NOW, store: area() } });
    await fireEvent.click(screen.getByRole("button", { name: /Move the selected thread/ }));
    const to = screen.getByLabelText("Move to page") as HTMLSelectElement;
    expect([...to.options].map(o => o.value)).toEqual([`${O}/billing/a-very-long-path-that-goes-on-and-on/and-on`, `${O}/users/:id`, `${O}/users/9`]);
    await fireEvent.change(to, { target: { value: `${O}/users/9` } });
    await fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(m.sent).toContainEqual({ t: "move", threadId: thread.id, pageUrl: `${O}/users/9` });
  });

  it("checks and previews a merge pattern, and sends nothing before the person confirms", async () => {
    const l = siteLink();
    const { container } = render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    const input = screen.getByLabelText("Pattern");
    await fireEvent.input(input, { target: { value: "/:a/:b" } });
    expect(screen.getByRole("alert").textContent).toMatch(/fixed segment/);
    expect((screen.getByRole("button", { name: "Merge pages" }) as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(input, { target: { value: "/users/:id" } });
    expect([...container.querySelectorAll(".paths li")].map(e => e.textContent)).toEqual(["/users/9"]);
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    // Nothing is sent before the person confirms, and Cancel sends nothing.
    expect(screen.getByText(/Merge 1 page \(0 threads\) into/)).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    // Enter in the pattern asks again.
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Yes, merge" })).toBeTruthy();
    expect(l.asked).toEqual([]);
  });

  it("runs the merge in batches with its progress, and un-merges", async () => {
    const l = siteLink([{ moved: 200, remaining: 3 }, { moved: 3, remaining: 0 }, { moved: 1, remaining: 0 }]);
    render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    await fireEvent.input(screen.getByLabelText("Pattern"), { target: { value: "/users/:id" } });
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    await fireEvent.click(screen.getByRole("button", { name: "Yes, merge" }));
    await settle();
    expect(l.asked).toEqual([{ t: "rule", origin: O, pattern: "/users/:id" }, { t: "rule", origin: O, pattern: "/users/:id" }]);
    expect(screen.getByText("Merged: 203 threads moved to /users/:id.")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Un-merge /users/:id" }));
    expect(screen.getByText(/Its 2 threads go back/)).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Yes, un-merge" }));
    await settle();
    expect(l.asked.at(-1)).toEqual({ t: "unrule", origin: O, ruleId: RULE });
    expect(screen.getByText("Un-merged /users/:id: 1 thread moved back.")).toBeTruthy();
  });

  it("says when the worker refuses a pattern to merge", async () => {
    render(Panel, { props: { link: siteLink() as never, now: NOW, store: area() } });
    await fireEvent.input(screen.getByLabelText("Pattern"), { target: { value: "/docs/*" } });
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    await fireEvent.click(screen.getByRole("button", { name: "Yes, merge" }));
    await settle();
    expect(screen.getByText("Not a pattern Clax can merge.")).toBeTruthy();
  });

  it.each([
    ["moves nothing", [{ moved: 0, remaining: 5 }]],
    ["does not lower the count", [{ moved: 2, remaining: 5 }, { moved: 1, remaining: 5 }]],
  ])("stops a merge whose batch %s, and says how many are left", async (_, steps) => {
    const l = siteLink([...steps, { moved: 1, remaining: 4 }]);
    render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    await fireEvent.input(screen.getByLabelText("Pattern"), { target: { value: "/users/:id" } });
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    await fireEvent.click(screen.getByRole("button", { name: "Yes, merge" }));
    await settle();
    expect(l.asked).toHaveLength(steps.length);
    expect(screen.getByText(/Stopped with 5 threads left/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Merge pages" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("stops a merge when the panel turns to another site, sending it nothing", async () => {
    let answer!: (s: { moved: number; remaining: number }) => void;
    const l = { ...siteLink(), request: (m: unknown) => { l.asked.push(m); return new Promise<{ moved: number; remaining: number }>(r => { answer = r; }); } };
    const r = render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    await fireEvent.input(screen.getByLabelText("Pattern"), { target: { value: "/users/:id" } });
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    await fireEvent.click(screen.getByRole("button", { name: "Yes, merge" }));
    expect(l.asked).toHaveLength(1);
    // The window's active tab is now one of another site.
    const B = "http://other.test:8080";
    await r.rerender({ link: { ...l, state: state({ url: `${B}/`, page: null, threads: [] }), site: { origin: B, rules: [], pages: [] } } as never, now: NOW, store: area() });
    answer({ moved: 200, remaining: 300 });
    await settle();
    expect(l.asked).toHaveLength(1);
    expect(screen.queryByText(/Merging/)).toBeNull();
  });
});

describe("Panel: joined sites", () => {
  const O = "http://localhost:5173";
  const A = "http://localhost:7702";
  const settle = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
  const NOW = new Date("2026-10-05T10:01:00.000Z");
  const lone = (): SiteView => ({ origin: O, site: { key: O, name: O, joined: false, origins: [{ origin: O, joined_at: null, last_used_at: null }] }, rules: [], pages: [] });
  const both = (): SiteView => ({ origin: O, site: { key: A, name: O, joined: true, origins: [{ origin: O, joined_at: "t", last_used_at: "t2" }, { origin: A, joined_at: "t", last_used_at: "t1" }] }, rules: [], pages: [] });
  function siteLink(site: SiteView, steps: { moved: number; remaining: number }[] = []) {
    const l = { ...link(state()), site, asked: [] as unknown[], suggestion: null as unknown, sites: null as unknown,
      request: async (m: unknown) => { l.asked.push(m); const r = steps.shift(); if (!r) throw new Error("No such site."); return r; } };
    return l;
  }

  it("asks once whether a lone origin is another site's app, and offers Join, Not now and Never", async () => {
    const l = siteLink(lone(), [{ moved: 200, remaining: 1 }, { moved: 1, remaining: 0 }]);
    const asked: string[][] = [];
    const r = render(Panel, { props: { link: l as never, now: NOW, permit: async (o: string[]) => { asked.push(o); return true; } } });
    expect(l.sent.filter(m => m.t === "suggest")).toHaveLength(1);
    expect(screen.queryByText(/same app\?/)).toBeNull();
    l.suggestion = { origin: O, suggestion: { origin: A, origins: [A], reason: "path", path: "/settings" } };
    await r.rerender({ link: { ...l } as never, now: NOW, permit: async (o: string[]) => { asked.push(o); return true; } });
    expect(screen.getByText(/Looks like/).textContent).toContain("localhost:7702");
    await fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await fireEvent.click(screen.getByRole("button", { name: "Never" }));
    expect(l.sent.filter(m => m.t === "answer")).toEqual([{ t: "answer", with: A, answer: "later" }, { t: "answer", with: A, answer: "never" }]);
    await fireEvent.click(within(screen.getByRole("group", { name: "Same app?" })).getByRole("button", { name: "Join" }));
    await settle();
    // Chrome is asked for the site's origins under the click, then the join runs in batches.
    expect(asked).toEqual([[A]]);
    expect(l.asked).toEqual([{ t: "join", origin: O, with: A }, { t: "join", origin: O, with: A }]);
    expect(screen.getByText(/Joined localhost:5173 and localhost:7702: one site, 201 threads merged/)).toBeTruthy();
  });

  it("joins nothing when Chrome is not allowed the other origin", async () => {
    const l = siteLink(lone());
    l.suggestion = { origin: O, suggestion: { origin: A, origins: [A], reason: "title", path: null } };
    render(Panel, { props: { link: l as never, now: NOW, permit: async () => false } });
    expect(screen.getByText(/page with this title/)).toBeTruthy();
    await fireEvent.click(within(screen.getByRole("group", { name: "Same app?" })).getByRole("button", { name: "Join" }));
    await settle();
    expect(l.asked).toEqual([]);
    expect(screen.getByText(/Chrome was not allowed access to localhost:7702/)).toBeTruthy();
  });

  it("lists a joined site's addresses with Split off, and joins the tab's origin to a site picked from the list", async () => {
    const l = siteLink(both(), [{ moved: 0, remaining: 0 }, { moved: 0, remaining: 0 }]);
    const r = render(Panel, { props: { link: l as never, now: NOW, permit: async () => true } });
    // A joined site is offered no suggestion.
    expect(l.sent.filter(m => m.t === "suggest")).toEqual([]);
    const tools = document.querySelector("details.addresses") as HTMLDetailsElement;
    tools.open = true;
    await fireEvent(tools, new Event("toggle"));
    expect(l.sent).toContainEqual({ t: "list-sites" });
    expect([...tools.querySelectorAll(".origins .o")].map(e => e.textContent)).toEqual(["localhost:5173", "localhost:7702"]);
    await fireEvent.click(screen.getByRole("button", { name: "Split localhost:7702 off" }));
    await settle();
    expect(l.asked).toEqual([{ t: "split", origin: A }]);
    expect(screen.getByText(/localhost:7702 is a site of its own again/)).toBeTruthy();
    // "Same app as…" offers the other sites only.
    const C = "http://localhost:3000";
    l.sites = [{ key: A, name: O, origins: [O, A] }, { key: C, name: C, origins: [C] }];
    await r.rerender({ link: { ...l } as never, now: NOW, permit: async () => true });
    const select = screen.getByLabelText("Same app as") as HTMLSelectElement;
    expect([...select.options].map(o => o.textContent)).toEqual(["Choose an address", "localhost:3000"]);
    await fireEvent.change(select, { target: { value: C } });
    await fireEvent.click(within(tools).getByRole("button", { name: "Join" }));
    await settle();
    expect(l.asked.at(-1)).toEqual({ t: "join", origin: O, with: C });
  });
});

describe("Panel: a join not finished", () => {
  const O = "http://localhost:5173";
  const A = "http://localhost:7702";
  const settle = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
  it("says how many threads are left and continues the join, asking Chrome for the site's origins", async () => {
    const site: SiteView = { origin: O, site: { key: A, name: O, joined: true, joining: 3, origins: [{ origin: O, joined_at: "t", last_used_at: "t2" }, { origin: A, joined_at: "t", last_used_at: "t1" }] }, rules: [], pages: [] };
    const asked: unknown[] = [];
    const permitted: string[][] = [];
    const l = { ...link(state()), site, request: async (m: unknown) => { asked.push(m); return { moved: 3, remaining: 0 }; } };
    render(Panel, { props: { link: l as never, permit: async (o: string[]) => { permitted.push(o); return true; } } });
    const box = screen.getByRole("group", { name: "Join not finished" });
    expect(box.textContent).toContain("3 threads left to merge");
    await fireEvent.click(within(box).getByRole("button", { name: "Continue joining" }));
    await settle();
    expect(permitted).toEqual([[O, A]]);
    expect(asked).toEqual([{ t: "join", origin: O, with: A }]);
  });

  it("continues the join even when Chrome is not allowed the other origins, saying Clax will not follow there", async () => {
    const site: SiteView = { origin: O, site: { key: A, name: O, joined: true, joining: 3, origins: [{ origin: O, joined_at: "t", last_used_at: "t2" }, { origin: A, joined_at: "t", last_used_at: "t1" }] }, rules: [], pages: [] };
    const asked: unknown[] = [];
    const l = { ...link(state()), site, request: async (m: unknown) => { asked.push(m); return { moved: 3, remaining: 0 }; } };
    render(Panel, { props: { link: l as never, permit: async () => false } });
    await fireEvent.click(within(screen.getByRole("group", { name: "Join not finished" })).getByRole("button", { name: "Continue joining" }));
    await settle();
    expect(asked).toEqual([{ t: "join", origin: O, with: A }]);
    expect(screen.getByText(/so Clax will not follow the tab there/).textContent).toContain("localhost:7702");
  });

  it("asks Chrome for the site's other origins before it opens a thread there", async () => {
    const T = "01J9BBBBBBBBBBBBBBBBBBBBBB";
    const far = { ...thread, id: T, artifact_id: "8r4m0nzy3c5v", page_path: "/b", page_url: `${A}/b` };
    const site: SiteView = { origin: O, site: { key: A, name: O, joined: true, origins: [{ origin: O, joined_at: "t", last_used_at: "t2" }, { origin: A, joined_at: "t", last_used_at: "t1" }] }, rules: [],
      pages: [{ page: { artifact_id: "8r4m0nzy3c5v", origin: A, path: "/b", page_url: `${A}/b`, title: "/b", current_version: 1, url: "http://localhost:7480/a/8r4m0nzy3c5v" }, threads: [far as never] }] };
    const permitted: string[][] = [];
    const l = { ...link(state()), site };
    const { container } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z"), permit: async (o: string[]) => { permitted.push(o); return true; } } });
    await fireEvent.click(within(container.querySelector(".far")!).getByRole("button", { name: "Go to page /b" }));
    await settle();
    expect(permitted).toEqual([[A]]);
    expect(l.sent).toContainEqual({ t: "open-thread", threadId: T });
  });
});
