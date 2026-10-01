import { describe, expect, it, vi } from "vitest";
import { FRAME_SANDBOX, FrameHost } from "./frame-host";

function stage() {
  const s = document.createElement("div");
  s.innerHTML = `<div class="island"></div>`;
  document.body.append(s);
  return s;
}

describe("FrameHost", () => {
  it("creates the frame first in the stage with the shell's attributes, and keeps it for the same key", () => {
    const s = stage();
    const host = new FrameHost(s, () => {});
    const el = host.show("/c/x/v/1/", true, "1-s");
    expect(s.firstElementChild).toBe(el);
    expect(el.className).toBe("frame");
    expect(el.title).toBe("artifact content");
    expect(el.getAttribute("allow")).toBe("clipboard-write; fullscreen");
    expect(el.getAttribute("sandbox")).toBe(FRAME_SANDBOX);
    expect(host.show("/c/x/v/1/", true, "1-s")).toBe(el);
    const next = host.show("http://x.localhost:1/v/2/", false, "2-o");
    expect(next).not.toBe(el);
    expect(el.isConnected).toBe(false);
    expect(next.hasAttribute("sandbox")).toBe(false);
  });
  it("adopts a frame already in the stage when its src and sandboxing match, and replaces it otherwise", () => {
    const s = stage();
    s.insertAdjacentHTML("afterbegin", `<iframe class="frame" title="artifact content" src="/c/x/v/1/" allow="clipboard-write; fullscreen" sandbox="${FRAME_SANDBOX}"></iframe>`);
    const served = s.querySelector("iframe")!;
    const onLoad = vi.fn();
    expect(new FrameHost(s, onLoad).show("/c/x/v/1/", true, "1-s")).toBe(served);
    served.dispatchEvent(new Event("load"));
    expect(onLoad).toHaveBeenCalledTimes(1);
    const other = stage();
    other.insertAdjacentHTML("afterbegin", `<iframe class="frame" title="artifact content" src="http://x.localhost:1/v/1/" allow="clipboard-write; fullscreen"></iframe>`);
    const stale = other.querySelector("iframe")!;
    const made = new FrameHost(other, () => {}).show("/c/x/v/1/", true, "1-s");
    expect(made).not.toBe(stale);
    expect(stale.isConnected).toBe(false);
  });
  it("adopts a served frame only with the shell's exact attributes, and sends it to the fragment", () => {
    const attrs = (sandbox: string | null, allow = "clipboard-write; fullscreen", title = "artifact content") =>
      `<iframe class="frame" title="${title}" src="/c/x/v/1/" allow="${allow}"${sandbox === null ? "" : ` sandbox="${sandbox}"`}></iframe>`;
    for (const [markup, sandboxed] of [
      [attrs(null), true],
      [attrs(`${FRAME_SANDBOX} allow-same-origin`), true],
      [attrs("allow-scripts"), true],
      [attrs(FRAME_SANDBOX, "clipboard-write; fullscreen; camera"), true],
      [attrs(FRAME_SANDBOX, "clipboard-write; fullscreen", "x"), true],
      [attrs(FRAME_SANDBOX), false],
    ] as const) {
      const s = stage();
      s.insertAdjacentHTML("afterbegin", markup);
      const served = s.querySelector("iframe")!;
      const el = new FrameHost(s, () => {}).show("/c/x/v/1/", sandboxed, "1");
      expect(el, markup).not.toBe(served);
      expect(served.isConnected).toBe(false);
      expect(el.getAttribute("sandbox")).toBe(sandboxed ? FRAME_SANDBOX : null);
    }
    const s = stage();
    s.insertAdjacentHTML("afterbegin", attrs(FRAME_SANDBOX));
    const served = s.querySelector("iframe")!;
    expect(new FrameHost(s, () => {}).show("/c/x/v/1/#part-2", true, "1")).toBe(served);
    expect(served.getAttribute("src")).toBe("/c/x/v/1/#part-2");
  });
  it("removes a served frame it did not adopt", () => {
    const s = stage();
    s.insertAdjacentHTML("afterbegin", `<iframe class="frame" src="/c/x/v/1/"></iframe>`);
    new FrameHost(s, () => {}).remove();
    expect(s.querySelector("iframe")).toBeNull();
  });
  it("sends a frame kept for the same key to a new src, and leaves it alone for the same src", () => {
    const s = stage();
    const host = new FrameHost(s, () => {});
    const el = host.show("/c/x/v/1/", true, "1-s");
    el.src = "/c/x/v/1/about.html";
    expect(host.show("/c/x/v/1/", true, "1-s")).toBe(el);
    expect(el.getAttribute("src")).toBe("/c/x/v/1/about.html");
    expect(host.show("/c/x/v/1/#moved", true, "1-s")).toBe(el);
    expect(el.getAttribute("src")).toBe("/c/x/v/1/#moved");
  });
  it("removes the frame and stops hearing its loads", () => {
    const s = stage();
    const onLoad = vi.fn();
    const host = new FrameHost(s, onLoad);
    const el = host.show("/c/x/v/1/", true, "k");
    host.remove();
    el.dispatchEvent(new Event("load"));
    expect(onLoad).not.toHaveBeenCalled();
    expect(host.el).toBeNull();
    expect(s.querySelector("iframe")).toBeNull();
  });
});
