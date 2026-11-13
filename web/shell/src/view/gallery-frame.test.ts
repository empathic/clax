import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FRAME_HTML, HEADER_HTML, SKELETON_HTML, replaceFrame } from "./gallery-frame";
import { MOUNT_TIMEOUT_MS } from "../test/timeouts";

/** The elements under `root` as tag, class and the attributes that place
 * or name them, in document order: what the static frame and the mounted
 * gallery must agree on. */
function shape(root: Element): string[] {
  const keep = ["type", "placeholder", "aria-label", "href", "role", "data-size"];
  return [root, ...root.querySelectorAll("*")].map(e => [e.localName, e.getAttribute("class") ?? "", ...keep.map(k => (e.hasAttribute(k) ? `${k}=${e.getAttribute(k)}` : ""))].join(" ").trim());
}

const parse = (html: string) => { const d = document.createElement("div"); d.innerHTML = html; return d; };

describe("index.html", () => {
  it("carries the gallery's frame in #app exactly, and the markers the daemon and the build fill", () => {
    const html = readFileSync(join(__dirname, "../../index.html"), "utf8");
    expect(html).toContain(`<div id="app">${FRAME_HTML}</div>`);
    expect(html).toContain("<!--clax:boot-->");
    expect(html).toContain("/*clax:preload*/{}");
    // The page's kind is marked after the block, which it reads, and before the body.
    expect(html.indexOf('<script id="clax-page">')).toBeGreaterThan(html.indexOf("<!--clax:boot-->"));
    expect(html.indexOf('<script id="clax-page">')).toBeLessThan(html.indexOf("<body>"));
  });
});

describe("the mounted gallery", { timeout: MOUNT_TIMEOUT_MS }, () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); history.replaceState(null, "", "/"); });

  for (const path of ["/", "/tags"]) {
    it(`renders the frame's header and skeleton until its answers are in (${path})`, async () => {
      history.replaceState(null, "", path);
      // Nothing answers: the gallery shows what it shows before any answer.
      vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(() => {})));
      const { default: Gallery } = await import("../ui/Gallery.svelte");
      const { mount } = await import("../test/svelte");
      const { root, unmount } = mount(Gallery, {});
      const frame = parse(path === "/" ? HEADER_HTML : HEADER_HTML.replace(/<input[^>]*>/, ""));
      expect(shape(root.querySelector(".gbar")!)).toEqual(shape(frame.querySelector(".gbar")!));
      expect(root.querySelector(".gbar .sub")!.textContent).toBe("");
      expect(root.querySelector("main.gal > .gskel")!.outerHTML).toBe(SKELETON_HTML);
      unmount();
    });
  }
});

describe("replaceFrame", () => {
  it("mounts before the frame and removes it in one go, carrying what was typed into its search box", () => {
    const root = document.createElement("div");
    document.body.append(root);
    root.innerHTML = FRAME_HTML;
    const footer = root.querySelector("nav.glegal");
    const typed = root.querySelector<HTMLInputElement>(".search")!;
    typed.value = "road";
    typed.focus();
    let heard = "";
    replaceFrame(root, anchor => {
      const fresh = parse(FRAME_HTML);
      fresh.querySelector("nav.glegal")!.remove();
      const search = fresh.querySelector<HTMLInputElement>(".search")!;
      search.addEventListener("input", () => { heard = search.value; });
      for (const n of [...fresh.childNodes]) root.insertBefore(n, anchor ?? null);
    });
    expect(root.querySelectorAll(".gbar")).toHaveLength(1);
    expect(root.querySelectorAll("main.gal")).toHaveLength(1);
    // The footer is the frame's own, kept, after what was mounted.
    expect(root.querySelectorAll("nav.glegal")).toHaveLength(1);
    expect(root.lastElementChild).toBe(footer);
    const now = root.querySelector<HTMLInputElement>(".search")!;
    expect(now).not.toBe(typed);
    expect(now.value).toBe("road");
    expect(heard).toBe("road");
    expect(document.activeElement).toBe(now);
    root.remove();
  });
});
