import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
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
    viewer: { public_id: "u_x", display_name: "Alex" }, commentMode: false, enabled: true, declined: false, selected: null, error: null, ...over,
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

  it("offers to start when Clax is off on the tab", () => {
    render(Panel, { props: { link: link(state({ enabled: false, page: null, threads: [] })) as never } });
    expect(screen.getByText(/Click the Clax button or press/)).toBeTruthy();
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

  it("goes to a thread's route when its card is on another route, then selects it", async () => {
    const other = { ...thread, anchor: { ...thread.anchor, route: "?tab=billing" } };
    const l = link(state({ threads: [other as never], resolved: {} }));
    const { container } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(container.textContent).toContain("?tab=billing");
    await fireEvent.click(container.querySelector(".card-head")!);
    expect(l.sent).toEqual([{ t: "navigate", route: "?tab=billing", artifactId: "7q3k9mzx2b4t" }, { t: "select", threadId: thread.id }]);
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

  it("says a reload will turn Clax off when the person refused the site's permission", async () => {
    const view = render(Panel, { props: { link: link(state()) as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(screen.queryByText(/turn off in this tab when the page reloads/)).toBeNull();
    await view.rerender({ link: link(state({ declined: true })) as never });
    expect(screen.getByText(/turn off in this tab when the page reloads/)).toBeTruthy();
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
    expect([...container.querySelectorAll(".far .body")].map(e => e.textContent)).toEqual(["Name wraps", "Avatar is blurry", "Old <img src=x onerror=alert(1)>"]);
    expect(container.querySelector(".far img")).toBeNull();
    expect(screen.getByText("at /users/2")).toBeTruthy();
    // A page with no threads is not listed.
    expect(container.textContent).not.toContain("/users/9");
  });

  it("opens a thread of another page in the tab, and says when its pin is on this screen", async () => {
    const l = siteLink();
    render(Panel, { props: { link: { ...l, state: { ...state(), resolved: { ...state().resolved, [T3]: { id: T3, found: true, method: "selector", rect: null } } } } as never, now: NOW, store: area() } });
    expect(screen.getAllByText("Pinned here")).toHaveLength(1);
    await fireEvent.click(screen.getByText("Avatar is blurry"));
    expect(l.sent).toContainEqual({ t: "open-thread", threadId: T3 });
  });

  it("filters both lists by status and searches them, remembering the filter for the site", async () => {
    const store = area();
    const { container } = render(Panel, { props: { link: siteLink() as never, now: NOW, store } });
    await fireEvent.click(screen.getByRole("button", { name: "Resolved" }));
    expect([...container.querySelectorAll(".far .body")].map(e => e.textContent)).toEqual(["Old <img src=x onerror=alert(1)>"]);
    expect(screen.queryByText("Too wide")).toBeNull();
    expect(screen.getByText("Nothing on this page matches.")).toBeTruthy();
    await settle();
    expect(store.data[`site-prefs:${O}`]).toEqual({ filter: "resolved", collapsed: [] });
    await fireEvent.click(screen.getByRole("button", { name: "All" }));
    await fireEvent.input(screen.getByLabelText("Search comments"), { target: { value: "users/3" } });
    expect([...container.querySelectorAll(".far .body")].map(e => e.textContent)).toEqual(["Name wraps"]);
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

  it("checks and previews a merge pattern, runs the merge in batches with its progress, and un-merges", async () => {
    const l = siteLink([{ moved: 200, remaining: 3 }, { moved: 3, remaining: 0 }, { moved: 1, remaining: 0 }]);
    const { container } = render(Panel, { props: { link: l as never, now: NOW, store: area() } });
    const input = screen.getByLabelText("Pattern");
    await fireEvent.input(input, { target: { value: "/:a/:b" } });
    expect(screen.getByRole("alert").textContent).toMatch(/fixed segment/);
    expect((screen.getByRole("button", { name: "Merge pages" }) as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(input, { target: { value: "/users/:id" } });
    expect([...container.querySelectorAll(".paths li")].map(e => e.textContent)).toEqual(["/users/9"]);
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    await settle();
    expect(l.asked).toEqual([{ t: "rule", pattern: "/users/:id" }, { t: "rule", pattern: "/users/:id" }]);
    expect(screen.getByText("Merged: 203 threads moved to /users/:id.")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Un-merge /users/:id" }));
    await settle();
    expect(l.asked.at(-1)).toEqual({ t: "unrule", ruleId: RULE });
    expect(screen.getByText("Un-merged /users/:id: 1 thread moved back.")).toBeTruthy();
    await fireEvent.input(input, { target: { value: "/docs/*" } });
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    await settle();
    expect(screen.getByText("Not a pattern Clax can merge.")).toBeTruthy();
  });
});
