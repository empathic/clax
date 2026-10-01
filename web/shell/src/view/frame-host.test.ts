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
    expect(host.show("/c/x/v/1/#moved", true, "1-s")).toBe(el);
    const next = host.show("http://x.localhost:1/v/2/", false, "2-o");
    expect(next).not.toBe(el);
    expect(el.isConnected).toBe(false);
    expect(next.hasAttribute("sandbox")).toBe(false);
  });
  it("adopts a frame already in the stage when its src and sandboxing match, and replaces it otherwise", () => {
    const s = stage();
    s.insertAdjacentHTML("afterbegin", `<iframe class="frame" src="/c/x/v/1/" sandbox="${FRAME_SANDBOX}"></iframe>`);
    const served = s.querySelector("iframe")!;
    const onLoad = vi.fn();
    expect(new FrameHost(s, onLoad).show("/c/x/v/1/", true, "1-s")).toBe(served);
    served.dispatchEvent(new Event("load"));
    expect(onLoad).toHaveBeenCalledTimes(1);
    const other = stage();
    other.insertAdjacentHTML("afterbegin", `<iframe class="frame" src="http://x.localhost:1/v/1/"></iframe>`);
    const stale = other.querySelector("iframe")!;
    const made = new FrameHost(other, () => {}).show("/c/x/v/1/", true, "1-s");
    expect(made).not.toBe(stale);
    expect(stale.isConnected).toBe(false);
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
