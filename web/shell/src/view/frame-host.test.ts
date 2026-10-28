import { describe, expect, it, vi } from "vitest";
import { INSPECT_FRAME } from "../../../bridge/src/protocol";
import { FrameHost } from "./frame-host";

const ALLOW = "clipboard-write; fullscreen; storage-access";
const START = "http://x.localhost:1/_clax/session/start?next=%2F_clax%2Fv%2F1%2F";

function stage() {
  const s = document.createElement("div");
  s.innerHTML = `<div class="island"></div>`;
  document.body.append(s);
  return s;
}

describe("FrameHost", () => {
  it("creates the frame first in the stage with the shell's attributes and no sandbox, and keeps it for the same key", () => {
    const s = stage();
    const host = new FrameHost(s, () => {});
    const el = host.show(START, "1");
    expect(s.firstElementChild).toBe(el);
    expect(el.className).toBe("frame");
    expect(el.title).toBe("artifact content");
    expect(el.getAttribute("allow")).toBe(ALLOW);
    expect(el.hasAttribute("sandbox")).toBe(false);
    expect(host.show(START, "1")).toBe(el);
    const next = host.show("http://x.localhost:1/_clax/session/start?next=%2F_clax%2Fv%2F2%2F", "2");
    expect(next).not.toBe(el);
    expect(el.isConnected).toBe(false);
  });
  it("adopts a served frame only with the shell's exact src and attributes, and replaces it otherwise", () => {
    const attrs = (src = START, allow = ALLOW, title = "artifact content", sandbox: string | null = null) =>
      `<iframe class="frame" title="${title}" src="${src}" allow="${allow}"${sandbox === null ? "" : ` sandbox="${sandbox}"`}></iframe>`;
    const s = stage();
    s.insertAdjacentHTML("afterbegin", attrs());
    const served = s.querySelector("iframe")!;
    const onLoad = vi.fn();
    expect(new FrameHost(s, onLoad).show(START, "1")).toBe(served);
    served.dispatchEvent(new Event("load"));
    expect(onLoad).toHaveBeenCalledTimes(1);
    for (const markup of [
      attrs("http://x.localhost:1/_clax/v/1/"),
      attrs(START, "clipboard-write; fullscreen"),
      attrs(START, `${ALLOW}; camera`),
      attrs(START, ALLOW, "x"),
      attrs(START, ALLOW, "artifact content", "allow-scripts allow-same-origin"),
    ]) {
      const t = stage();
      t.insertAdjacentHTML("afterbegin", markup);
      const odd = t.querySelector("iframe")!;
      const el = new FrameHost(t, () => {}).show(START, "1");
      expect(el, markup).not.toBe(odd);
      expect(odd.isConnected).toBe(false);
      expect(el.hasAttribute("sandbox")).toBe(false);
    }
  });
  it("names the frame for inspection in the app's inspect view, and adopts a served frame there only with that name", () => {
    const served = (name: string | null) => {
      const s = stage();
      s.insertAdjacentHTML("afterbegin", `<iframe class="frame"${name === null ? "" : ` name="${name}"`} title="artifact content" src="${START}" allow="${ALLOW}"></iframe>`);
      return { s, el: s.querySelector("iframe")! };
    };
    history.replaceState(null, "", "/a/x?app=1&inspect=1");
    try {
      const named = served(INSPECT_FRAME);
      expect(new FrameHost(named.s, () => {}).show(START, "1")).toBe(named.el);
      const unnamed = served(null);
      const made = new FrameHost(unnamed.s, () => {}).show(START, "1");
      expect(made).not.toBe(unnamed.el);
      expect(made.name).toBe(INSPECT_FRAME);
    } finally {
      history.replaceState(null, "", "/");
    }
    // Outside the inspect view, a named served frame is not the shell's.
    const named = served(INSPECT_FRAME);
    expect(new FrameHost(named.s, () => {}).show(START, "1")).not.toBe(named.el);
    expect(new FrameHost(stage(), () => {}).show(START, "1").name).toBe("");
  });
  it("removes a served frame it did not adopt", () => {
    const s = stage();
    s.insertAdjacentHTML("afterbegin", `<iframe class="frame" src="${START}"></iframe>`);
    new FrameHost(s, () => {}).remove();
    expect(s.querySelector("iframe")).toBeNull();
  });
  it("sends a frame kept for the same key to a new src, and leaves it alone for the same src", () => {
    const s = stage();
    const host = new FrameHost(s, () => {});
    const el = host.show(START, "1");
    el.src = "http://x.localhost:1/_clax/v/1/about.html";
    expect(host.show(START, "1")).toBe(el);
    expect(el.getAttribute("src")).toBe("http://x.localhost:1/_clax/v/1/about.html");
    const moved = `${START}%23moved`;
    expect(host.show(moved, "1")).toBe(el);
    expect(el.getAttribute("src")).toBe(moved);
  });
  it("goes to the handshake's next step in place of the frame's history entry, keeping the src it was given, and back to that src again", () => {
    const s = stage();
    const host = new FrameHost(s, () => {});
    const el = host.show(START, "1");
    const replace = vi.fn();
    Object.defineProperty(el, "contentWindow", { configurable: true, get: () => ({ location: { replace } }) });
    const code = "http://x.localhost:1/_clax/session?code=c&next=%2F";
    host.go(code);
    expect(replace).toHaveBeenLastCalledWith(code);
    expect(el.getAttribute("src")).toBe(START);
    // The same src again leaves the frame where the handshake sent it.
    expect(host.show(START, "1")).toBe(el);
    expect(replace).toHaveBeenCalledTimes(1);
    host.again();
    expect(replace).toHaveBeenLastCalledWith(START);
  });
  it("falls back to the frame's src when its window cannot be navigated in place", () => {
    const s = stage();
    const host = new FrameHost(s, () => {});
    const el = host.show(START, "1");
    Object.defineProperty(el, "contentWindow", { configurable: true, get: () => ({ location: { replace() { throw new DOMException("no", "SecurityError"); } } }) });
    const code = "http://x.localhost:1/_clax/session?code=c&next=%2F";
    host.go(code);
    expect(el.getAttribute("src")).toBe(code);
    host.again();
    expect(el.getAttribute("src")).toBe(START);
  });
  it("removes the frame and stops hearing its loads", () => {
    const s = stage();
    const onLoad = vi.fn();
    const host = new FrameHost(s, onLoad);
    const el = host.show(START, "k");
    host.remove();
    el.dispatchEvent(new Event("load"));
    expect(onLoad).not.toHaveBeenCalled();
    expect(host.el).toBeNull();
    expect(s.querySelector("iframe")).toBeNull();
    host.go("x");
    host.again();
  });
});

describe("FrameHost on a path", () => {
  it("sandboxes a frame on a path without allow-same-origin, and adopts a served one only with that sandbox", () => {
    const src = "/_clax/frame/7q3k9mzx2b4t/_clax/v/1/";
    const sandbox = "allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals";
    const s = stage();
    const el = new FrameHost(s, () => {}).show(src, "1");
    expect(el.getAttribute("sandbox")).toBe(sandbox);
    expect(el.getAttribute("sandbox")).not.toContain("allow-same-origin");
    const t = stage();
    t.insertAdjacentHTML("afterbegin", `<iframe class="frame" title="artifact content" src="${src}" allow="${ALLOW}" sandbox="${sandbox}"></iframe>`);
    const served = t.querySelector("iframe")!;
    expect(new FrameHost(t, () => {}).show(src, "1")).toBe(served);
    const u = stage();
    u.insertAdjacentHTML("afterbegin", `<iframe class="frame" title="artifact content" src="${src}" allow="${ALLOW}"></iframe>`);
    const bare = u.querySelector("iframe")!;
    expect(new FrameHost(u, () => {}).show(src, "1")).not.toBe(bare);
  });
});
