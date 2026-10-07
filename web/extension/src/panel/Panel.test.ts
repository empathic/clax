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

  it("shows an action's failure again after a dismissal when the action fails again", async () => {
    const error = { code: "no_capture_permission", message: "Clax needs a click on its button to take screenshots on this tab." };
    const l = { ...link(state({ error })), failures: 1 };
    const { rerender } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    // In a tab Clax is on, its button turns Clax off: the panel names the command instead.
    expect(screen.getByRole("alert").textContent).toContain("Press ⌥⇧C");
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
    // Nothing is sent before the person confirms, and Cancel sends nothing.
    expect(screen.getByText(/Merge 1 page \(0 threads\) into/)).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(l.asked).toEqual([]);
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
    await fireEvent.input(input, { target: { value: "/docs/*" } });
    await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
    await fireEvent.click(screen.getByRole("button", { name: "Yes, merge" }));
    await settle();
    expect(screen.getByText("Not a pattern Clax can merge.")).toBeTruthy();
  });

  it("stops a merge whose batches move nothing, or whose count does not fall, and says how many are left", async () => {
    for (const steps of [[{ moved: 0, remaining: 5 }], [{ moved: 2, remaining: 5 }, { moved: 1, remaining: 5 }]]) {
      const l = siteLink([...steps, { moved: 1, remaining: 4 }]);
      render(Panel, { props: { link: l as never, now: NOW, store: area() } });
      await fireEvent.input(screen.getByLabelText("Pattern"), { target: { value: "/users/:id" } });
      await fireEvent.click(screen.getByRole("button", { name: "Merge pages" }));
      await fireEvent.click(screen.getByRole("button", { name: "Yes, merge" }));
      await settle();
      expect(l.asked).toHaveLength(steps.length);
      expect(screen.getByText(/Stopped with 5 threads left/)).toBeTruthy();
      expect((screen.getByRole("button", { name: "Merge pages" }) as HTMLButtonElement).disabled).toBe(false);
      cleanup();
    }
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
    await fireEvent.click(container.querySelector("article.far button.go")!);
    await settle();
    expect(permitted).toEqual([[A]]);
    expect(l.sent).toContainEqual({ t: "open-thread", threadId: T });
  });
});
