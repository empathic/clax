import { afterEach, describe, expect, it, vi } from "vitest";
import { artifact, artifactViewHooks, mountView, waitFor } from "../test/artifact-view";
import { flush, mount } from "../test/svelte";
import { MOUNT_TIMEOUT_MS } from "../test/timeouts";
import PhoneTabs from "./PhoneTabs.svelte";
import Publish from "./Publish.svelte";
import SendButton from "./SendButton.svelte";
import Sidebar from "./Sidebar.svelte";
import VersionMenu from "./VersionMenu.svelte";
import { SHELL_CONTROLS, findControl, isShellControl, isShown } from "./shell-controls";

const marked = (name: string, host: HTMLElement = document.body, tag = "button") => {
  const el = document.createElement(tag);
  el.setAttribute("data-shell-control", name);
  host.appendChild(el);
  return el;
};

afterEach(() => { document.body.replaceChildren(); });

describe("the shell's marked controls", () => {
  it("names six controls and recognises only them", () => {
    expect([...SHELL_CONTROLS]).toEqual(["comment", "threads", "versions", "publish", "fork", "send"]);
    expect(isShellControl("fork")).toBe(true);
    expect(isShellControl("share")).toBe(false);
    expect(isShellControl(undefined)).toBe(false);
  });

  it("finds_the_first_shown_element_for_a_name", () => {
    const topBar = marked("threads");
    const phoneTab = marked("threads");
    expect(findControl("threads", document, el => el === phoneTab)).toBe(phoneTab);
    expect(findControl("threads", document, () => true)).toBe(topBar);
  });

  it("a_disabled_or_inert_control_is_not_shown", () => {
    const rects = stubProto(Element.prototype, "getClientRects", () => [{}] as unknown as DOMRectList);
    const visible = stubProto(Element.prototype, "checkVisibility", () => true);
    try {
      const plain = marked("comment");
      expect(isShown(plain)).toBe(true);
      const disabled = marked("comment") as HTMLButtonElement;
      disabled.disabled = true;
      expect(isShown(disabled)).toBe(false);
      const inert = document.createElement("div");
      inert.setAttribute("inert", "");
      document.body.appendChild(inert);
      expect(isShown(marked("comment", inert))).toBe(false);
      const hidden = document.createElement("div");
      hidden.setAttribute("aria-hidden", "true");
      document.body.appendChild(hidden);
      expect(isShown(marked("comment", hidden))).toBe(false);
      const detached = document.createElement("button");
      expect(isShown(detached)).toBe(false);
    } finally {
      rects(); visible();
    }
  });

  it("an element with no box, no visibility or no opacity is not shown", () => {
    const el = marked("comment");
    expect(isShown(el)).toBe(false); // jsdom lays nothing out: no client rects
    const rects = stubProto(Element.prototype, "getClientRects", () => [{}] as unknown as DOMRectList);
    let visible = true;
    const cv = stubProto(Element.prototype, "checkVisibility", () => visible);
    try {
      expect(isShown(el)).toBe(true);
      visible = false;
      expect(isShown(el)).toBe(false);
    } finally { cv(); }
    // Without checkVisibility the computed style decides.
    const none = stubProto(Element.prototype, "checkVisibility", undefined);
    try {
      expect(isShown(el)).toBe(true);
      el.style.visibility = "hidden";
      expect(isShown(el)).toBe(false);
      el.style.visibility = "";
      el.style.opacity = "0";
      expect(isShown(el)).toBe(false);
    } finally { none(); rects(); }
  });

  it("no_marked_element_is_null", () => {
    expect(findControl("fork")).toBeNull();
  });

  describe("every_name_is_marked_somewhere", { timeout: MOUNT_TIMEOUT_MS }, () => {
    artifactViewHooks();

    const owner = async (links: unknown[] = []) => {
      (await import("./more-menu.svelte")).loadMoreMenu();
      return mountView(async url => {
        if (url.endsWith("/remotes")) return new Response(JSON.stringify({ links, remotes: [] }));
        return new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1)));
      });
    };
    const only = (root: Element, name: string) => {
      const all = root.querySelectorAll<HTMLElement>(`[data-shell-control="${name}"]`);
      expect(all).toHaveLength(1);
      return all[0];
    };

    it("marks the top bar's Comment, Threads, version and Fork buttons, each on its own button", async () => {
      const view = await owner();
      for (const name of ["comment", "threads", "versions", "fork"]) {
        await waitFor(() => view.root.querySelector(`[data-shell-control="${name}"]`), `the ${name} control`);
      }
      const bar = view.root;
      expect(only(bar, "comment").matches("button.comment")).toBe(true);
      expect(only(bar, "comment").textContent).toContain("Comment");
      // Threads is marked twice: the top bar's button and the phone's tab, in that order.
      const threads = [...bar.querySelectorAll<HTMLElement>('[data-shell-control="threads"]')];
      expect(threads.map(t => t.matches("button.threads") ? "bar" : t.closest(".phone-tabs") ? "tab" : "other")).toEqual(["bar", "tab"]);
      expect(threads[0].textContent).toContain("Threads");
      expect(only(bar, "versions").matches("button.vbtn")).toBe(true);
      expect(only(bar, "fork").matches("button.fork")).toBe(true);
      expect(only(bar, "fork").textContent).toBe("Fork");
    });

    it("finds the mounted Comment button, and not while it is disabled", async () => {
      const view = await owner();
      const comment = await waitFor(() => view.root.querySelector<HTMLButtonElement>('button[data-shell-control="comment"]'), "Comment");
      const rects = stubProto(Element.prototype, "getClientRects", () => [{}] as unknown as DOMRectList);
      const visible = stubProto(Element.prototype, "checkVisibility", () => true);
      try {
        comment.disabled = false;
        expect(findControl("comment")).toBe(comment);
        comment.disabled = true;
        expect(findControl("comment")).toBeNull();
      } finally { rects(); visible(); }
    });

    it("marks the phone tab, the version button and the send button", () => {
      const tabs = mount(PhoneTabs, { panel: false, open: 0, onPage() {}, onThreads() {} });
      expect(tabs.root.querySelector('button[data-shell-control="threads"]')!.textContent).toContain("Threads");
      const menu = mount(VersionMenu, { shown: 1, latest: 1, dot: false, open: false, onToggle() {}, input: () => ({}) as never, hrefFor: () => "", onChoose() {} });
      expect(menu.root.querySelector('button.vbtn[data-shell-control="versions"]')).not.toBeNull();
      const send = mount(SendButton, { label: "Send", agents: [], target: null, onSend() {}, onChoose() {} });
      const b = send.root.querySelector('[data-shell-control="send"]')!;
      expect(b.matches("button.primary:not(.caret)")).toBe(true);
      expect(b.textContent).toBe("Send");
    });

    describe("Publish", () => {
      const settle = async () => { for (let i = 0; i < 5; i++) { await new Promise(r => setTimeout(r)); flush(); } };
      const daemon = (links: unknown[]) => vi.stubGlobal("fetch", vi.fn(async (url: string) => {
        if (url === "/api/token") return new Response(JSON.stringify({ token: "t-local" }));
        if (url.endsWith("/remotes")) return new Response(JSON.stringify({ links, remotes: [{ id: "r_1", origin: "https://clax.example", name: "clax.example", status: "connected", person_name: "Ada" }] }));
        return new Response("{}", { status: 404 });
      }));
      afterEach(() => vi.unstubAllGlobals());

      it("marks the Publish button, and the Linked button in the linked state", async () => {
        daemon([]);
        const m = mount(Publish, { id: "a1", title: "Plan", latest: 2 });
        await settle();
        const marks = m.root.querySelectorAll('[data-shell-control="publish"]');
        expect(marks).toHaveLength(1);
        expect(marks[0].matches("button.publish")).toBe(true);
        m.unmount();
        daemon([{ remote_id: "r_1", remote_name: "clax.example", origin: "https://clax.example", url: "https://clax.example/a/b1", pushed_n: 2, remote_n: 2, auto: false, behind: 0 }]);
        const l = mount(Publish, { id: "a1", title: "Plan", latest: 2 });
        await settle();
        const linked = l.root.querySelectorAll('[data-shell-control="publish"]');
        expect(linked).toHaveLength(1);
        expect(linked[0].matches("button.linked")).toBe(true);
        expect(linked[0].textContent).toContain("Linked");
        l.unmount();
      });
    });

    it("marks the unsent bar's send button in the sidebar", () => {
      const base = { artifact_id: "7q3k9mzx2b4t", version_n: 1, has_clip: false, clip_url: null, created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null };
      const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
      const threads = ["a", "b"].map(id => ({ ...base, id, anchor, status: "open" as const, sent_to_agent: false, comments: [{ id: `${id}1`, thread_id: id, author_kind: "viewer", author_name: "Alex", via_harness: null, body: "x", created_at: base.created_at }] }));
      const agents = [{ handle: "a_cx", harness: "codex", live: true }, { handle: "a_cl", harness: "claude", live: true }];
      const m = mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads, resolved: {}, now: new Date(base.created_at), selected: null, sendTo: "a_cx", agents,
        onSelect() {}, onSend() {}, onResolve() {}, onReply() {}, selection: { ids: [], anchor: null }, onToggle() {}, onChoose() {}, onSendUnsent() {} } as never);
      // Each card's Send is marked too; the bar's is the one that is a .send-unsent button.
      const bar = m.root.querySelectorAll<HTMLElement>("button.send-unsent");
      expect(bar).toHaveLength(1);
      expect(bar[0].getAttribute("data-shell-control")).toBe("send");
      expect(bar[0].textContent).toBe("Send 2 unsent to codex");
      m.unmount();
    });
  });
});

/** Replaces `proto[key]` for a test (`undefined` removes it); returns the restore. */
function stubProto(proto: object, key: string, fn: (() => unknown) | undefined): () => void {
  const had = Object.getOwnPropertyDescriptor(proto, key);
  if (fn === undefined) Object.defineProperty(proto, key, { configurable: true, writable: true, value: undefined });
  else Object.defineProperty(proto, key, { configurable: true, writable: true, value: fn });
  return () => { if (had) Object.defineProperty(proto, key, had); else delete (proto as Record<string, unknown>)[key]; };
}
