import { afterEach, describe, expect, it, vi } from "vitest";
import { CHROME_EVERY_MS, reportChrome, sampleChrome } from "../src/chrome";

function fakeTimers() {
  let next = 1;
  const due = new Map<number, () => void>();
  return {
    t: { set: (fn: () => void) => { due.set(next, fn); return next++; }, clear: (id: number) => { due.delete(id); } },
    fire: () => { const all = [...due.values()]; due.clear(); all.forEach(fn => fn()); },
    pending: () => due.size,
  };
}

/** Lays out `html` in the body and answers the element `top` at the
 * viewport's top edge and `bottom` at its bottom edge (jsdom has no
 * layout, so no `elementFromPoint`). */
function page(html: string, css = "") {
  document.head.innerHTML = `<style>${css}</style>`;
  document.body.innerHTML = html;
  return (top: string | null, bottom: string | null) => (_x: number, y: number) =>
    (y === 0 ? (top && document.querySelector(top)) : (bottom && document.querySelector(bottom))) || null;
}

afterEach(() => {
  document.head.innerHTML = "";
  document.body.innerHTML = "";
  document.body.removeAttribute("style");
  document.documentElement.removeAttribute("style");
});

describe("sampleChrome", () => {
  it("takes the background of the element at each edge", () => {
    const at = page("<header>Runs</header><main>…</main><nav>Tabs</nav>", "header { background: #102030 } nav { background-color: rgb(250, 250, 250) }");
    expect(sampleChrome(window, at("header", "nav"))).toEqual({ top: "#102030", bottom: "#fafafa" });
  });

  it("walks up past transparent elements to the body's colour", () => {
    const at = page("<div class=wrap><header><h1>Runs</h1></header></div>", "body { background: #223344 } header { background: transparent }");
    expect(sampleChrome(window, at("h1", "h1"))).toEqual({ top: "#223344", bottom: "#223344" });
  });

  it("skips a colour that is not opaque", () => {
    const at = page("<header>Runs</header>", "html { background: #0a0b0c } header { background: rgba(255, 0, 0, 0.5) }");
    expect(sampleChrome(window, at("header", null))).toEqual({ top: "#0a0b0c", bottom: "#0a0b0c" });
  });

  it("reads a colour mixed in sRGB", () => {
    const at = page("<header>Runs</header>", "header { background-color: color(srgb 1 0.5 0) }");
    // jsdom keeps color() as written; browsers compute color-mix(in srgb …) to it.
    expect(sampleChrome(window, at("header", null)).top).toBe("#ff8000");
  });

  it("is white when nothing has a background", () => {
    const at = page("<p>Hello</p>");
    expect(sampleChrome(window, at("p", null))).toEqual({ top: "#ffffff", bottom: "#ffffff" });
  });
});

describe("reportChrome", () => {
  it("posts the edges once at first, then again only when they change", async () => {
    const f = fakeTimers();
    const post = vi.fn();
    const at = page("<header>Runs</header>", "header { background: #102030 }");
    reportChrome(window, post, f.t, at("header", null));
    f.fire();
    expect(post.mock.calls).toEqual([[{ type: "clax:chrome", top: "#102030", bottom: "#ffffff" }]]);
    // A scroll that leaves the edges as they were posts nothing.
    window.dispatchEvent(new Event("scroll"));
    f.fire();
    expect(post).toHaveBeenCalledTimes(1);
    // A change to the page, sampled once however many changes come.
    document.querySelector("header")!.setAttribute("style", "background: #ffffff");
    document.body.setAttribute("style", "background: #000000");
    await Promise.resolve();
    window.dispatchEvent(new Event("resize"));
    expect(f.pending()).toBe(1);
    f.fire();
    expect(post.mock.calls.at(-1)).toEqual([{ type: "clax:chrome", top: "#ffffff", bottom: "#000000" }]);
    expect(post).toHaveBeenCalledTimes(2);
    expect(CHROME_EVERY_MS).toBeGreaterThan(0);
  });

  it("samples again when the light or dark appearance changes", () => {
    const f = fakeTimers();
    const post = vi.fn();
    // A page whose colours follow prefers-color-scheme changes no DOM when
    // the appearance flips: only the media query says so.
    let listener: (() => void) | null = null;
    const query = { matches: false, addEventListener: (_: string, fn: () => void) => { listener = fn; } };
    const media = vi.spyOn(window, "matchMedia").mockImplementation(q => {
      expect(q).toBe("(prefers-color-scheme: dark)");
      return query as unknown as MediaQueryList;
    });
    try {
      const at = page("<header>Runs</header>", "header { background: #ffffff }");
      reportChrome(window, post, f.t, at("header", null));
      f.fire();
      expect(post).toHaveBeenCalledTimes(1);
      document.head.innerHTML = "<style>header { background: #1d1d1f }</style>";
      listener!();
      expect(f.pending()).toBe(1);
      f.fire();
      expect(post.mock.calls.at(-1)).toEqual([{ type: "clax:chrome", top: "#1d1d1f", bottom: "#ffffff" }]);
    } finally {
      media.mockRestore();
    }
  });
});
