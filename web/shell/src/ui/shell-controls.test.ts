import { afterEach, describe, expect, it } from "vitest";
import { ID, artifact, artifactViewHooks, mountView, waitFor } from "../test/artifact-view";
import { mount } from "../test/svelte";
import { MOUNT_TIMEOUT_MS } from "../test/timeouts";
import PhoneTabs from "./PhoneTabs.svelte";
import publishSrc from "./Publish.svelte?raw";
import SendButton from "./SendButton.svelte";
import sidebarSrc from "./Sidebar.svelte?raw";
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
    const rects = vi_stub(Element.prototype, "getClientRects", () => [{}] as unknown as DOMRectList);
    const visible = vi_stub(Element.prototype, "checkVisibility", () => true);
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

  it("an element with no box or no visibility is not shown", () => {
    const el = marked("comment");
    expect(isShown(el)).toBe(false); // jsdom lays nothing out: no client rects
  });

  it("no_marked_element_is_null", () => {
    expect(findControl("fork")).toBeNull();
  });

  describe("every_name_is_marked_somewhere", { timeout: MOUNT_TIMEOUT_MS }, () => {
    artifactViewHooks();

    it("marks the top bar's controls", async () => {
      (await import("./more-menu.svelte")).loadMoreMenu();
      const view = await mountView(async url => {
        if (url.endsWith("/remotes")) return new Response(JSON.stringify({ error: { code: "forbidden_origin", message: "nope" } }), { status: 403 });
        return new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : artifact(1)));
      });
      for (const name of ["comment", "threads", "versions", "fork"]) {
        await waitFor(() => view.root.querySelector(`[data-shell-control="${name}"]`), `the ${name} control`);
      }
      expect(ID).toBeTruthy();
    });

    it("marks the phone tab, the version button and the send button", () => {
      const tabs = mount(PhoneTabs, { panel: false, open: 0, onPage() {}, onThreads() {} });
      expect(tabs.root.querySelector('[data-shell-control="threads"]')!.textContent).toContain("Threads");
      const menu = mount(VersionMenu, { shown: 1, latest: 1, dot: false, open: false, onToggle() {}, input: () => ({}) as never, hrefFor: () => "", onChoose() {} });
      expect(menu.root.querySelector('[data-shell-control="versions"]')).not.toBeNull();
      const send = mount(SendButton, { label: "Send", agents: [], target: null, onSend() {}, onChoose() {} });
      expect(send.root.querySelector('[data-shell-control="send"]')).not.toBeNull();
    });

    it("marks both of Publish's buttons", () => {
      expect(publishSrc.split('data-shell-control="publish"').length - 1).toBe(2);
    });

    it("marks the unsent bar's button in the sidebar", () => {
      expect(sidebarSrc.split('data-shell-control="send"').length - 1).toBe(1);
    });
  });
});

/** Replaces `proto[key]` for a test; returns the restore. */
function vi_stub(proto: object, key: string, fn: () => unknown): () => void {
  const had = Object.getOwnPropertyDescriptor(proto, key);
  Object.defineProperty(proto, key, { configurable: true, writable: true, value: fn });
  return () => { if (had) Object.defineProperty(proto, key, had); else delete (proto as Record<string, unknown>)[key]; };
}
