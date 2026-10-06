import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, describe, expect, it } from "vitest";
import type { PanelState, PanelToWorker } from "../messages";
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
    expect(l.sent).toEqual([{ t: "navigate", route: "?tab=billing" }, { t: "select", threadId: thread.id }]);
  });

  it("shows who is here on the page beside its agents", () => {
    const l = link(state({ presence: [{ public_id: "u_y", display_name: "Mia Wong", state: "here", where: null, since: "2026-10-05T10:00:00.000Z" }] }));
    const { container } = render(Panel, { props: { link: l as never, now: new Date("2026-10-05T10:01:00.000Z") } });
    expect(container.querySelector(".tok.p.here")?.textContent).toBe("MW");
    expect(container.querySelector(".tok.a")?.textContent).toBe("cl");
  });
});
