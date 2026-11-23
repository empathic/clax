import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AUTH_FRAME_HTML, FRAME_HTML, HEADER_HTML, LOADING, NEEDS_HEADING, NEEDS_RULE, REST_AFTER_NEEDS, REST_HEADING, REST_NOTE, SKELETON_CARDS, SKELETON_HTML, replaceFrame } from "./gallery-frame";
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
    expect(html).toContain(`<div id="app">${FRAME_HTML}${AUTH_FRAME_HTML}</div>`);
    // The script that shows `#app` (gallery.css) once it is parsed whole comes right after it.
    expect(html).toContain(`${AUTH_FRAME_HTML}</div>\n<script id="clax-framed">`);
    expect(html).toContain("<!--clax:boot-->");
    expect(html).toContain("/*clax:preload*/{}");
    // The page's kind is marked after the block, which it reads, and before the body.
    expect(html.indexOf('<script id="clax-page">')).toBeGreaterThan(html.indexOf("<!--clax:boot-->"));
    expect(html.indexOf('<script id="clax-page">')).toBeLessThan(html.indexOf("<body>"));
    // The skeleton is shaped after both, before the body.
    expect(html.indexOf('<script id="clax-shape">')).toBeGreaterThan(html.indexOf('<script id="clax-page">'));
    expect(html.indexOf('<script id="clax-shape">')).toBeLessThan(html.indexOf("<body>"));
  });

  it("holds every part the content may have in the skeleton, in the content's own boxes, and one polite status", () => {
    const sk = parse(SKELETON_HTML);
    for (const part of [".sk-ban", ".sk-sum", ".sk-tb", ".sk-empty", "section.grp.needs.sk-need > h2", "section.grp.needs.sk-need > p.rule", "section.grp.rest.sk-rest > h2", ".sk-ls"]) expect(sk.querySelector(part), part).not.toBeNull();
    expect(sk.querySelectorAll(".sk-need .cards > .sk-c")).toHaveLength(SKELETON_CARDS);
    expect(sk.querySelectorAll(".sk-rest .cards > .sk-c")).toHaveLength(SKELETON_CARDS);
    expect([...sk.querySelectorAll("[role=status]")].map(e => e.textContent)).toEqual([LOADING]);
    expect(sk.querySelector(".sk")!.getAttribute("aria-hidden")).toBe("true");
    expect(sk.querySelector(".sk-need h2")!.textContent).toBe(NEEDS_HEADING);
    expect(sk.querySelector(".sk-need .rule")!.textContent).toBe(NEEDS_RULE);
    expect(sk.querySelector(".sk-rest h2")!.textContent).toBe(`${REST_HEADING}${REST_AFTER_NEEDS}${REST_NOTE}`);
    expect(parse(FRAME_HTML).querySelector("main.gal")!.getAttribute("aria-busy")).toBe("true");
  });
});

describe("the sign-in pages' frame", () => {
  // The entry's inline script after `#app`, run on the frame for a page kind.
  const html = readFileSync(join(__dirname, "../../index.html"), "utf8");
  const script = /<script id="clax-framed">([\s\S]*?)<\/script>/.exec(html)![1];
  const framed = (auth: string, signed = false) => {
    document.documentElement.dataset.auth = auth;
    if (signed) document.documentElement.dataset.signed = ""; else delete document.documentElement.dataset.signed;
    document.body.innerHTML = `<div id="app">${AUTH_FRAME_HTML}</div>`;
    new Function(script)();
    return document.querySelector<HTMLElement>("#app > .aframe")!;
  };
  afterEach(() => { delete document.documentElement.dataset.auth; delete document.documentElement.dataset.signed; document.body.replaceChildren(); document.title = ""; });

  it("heads each sign-in page's card with the page's own heading, over the skeleton, and says it is loading", () => {
    for (const [auth, title] of [["setup", "Set up Clax"], ["join", "Join Clax"], ["signin", "Sign in"], ["device", "Connect a Clax"], ["status", "Sign in"]] as const) {
      const f = framed(auth);
      expect(f.querySelector("main.signin > .sicard > h2")!.textContent, auth).toBe(title);
      expect(document.title).toBe(`${title} · Clax`);
      expect(f.querySelector("main.signin")!.getAttribute("aria-busy")).toBe("true");
      expect([...f.querySelectorAll("[role=status]")].map(e => e.textContent)).toEqual([LOADING]);
      expect(f.querySelector("iframe")).toBeNull();
    }
  });

  it("starts the landing page's quickstart in its stage, under a Sign in link, or Your gallery once signed in", () => {
    let f = framed("landing");
    expect(f.querySelector("iframe.lframe")!.getAttribute("src")).toBe("/start/index.html");
    expect(f.lastElementChild).toBe(f.querySelector("iframe.lframe"));
    expect(f.querySelector<HTMLAnchorElement>(".gbar a.lgo")!.textContent).toBe("Sign in");
    expect(f.querySelector<HTMLAnchorElement>(".gbar a.lgo")!.getAttribute("href")).toBe("/signin");
    f = framed("landing", true);
    expect(f.querySelector<HTMLAnchorElement>(".gbar a.lgo")!.textContent).toBe("Your gallery");
    expect(f.querySelector<HTMLAnchorElement>(".gbar a.lgo")!.getAttribute("href")).toBe("/");
  });

  it("does nothing on the gallery's pages", () => {
    const f = framed("");
    expect(f.querySelector("h2")!.textContent).toBe("");
    expect(f.querySelector("iframe")).toBeNull();
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
      // The entry's static frame, whose skeleton the gallery shows as its own.
      const app = Object.assign(document.createElement("div"), { id: "app", innerHTML: FRAME_HTML });
      document.body.append(app);
      const { root, unmount } = mount(Gallery, {});
      const frame = parse(path === "/" ? HEADER_HTML : HEADER_HTML.replace(/<input[^>]*>/, ""));
      expect(shape(root.querySelector(".gbar")!)).toEqual(shape(frame.querySelector(".gbar")!));
      expect(root.querySelector(".gbar .sub")!.textContent).toBe("");
      expect(root.querySelector("main.gal > .gskel")!.outerHTML).toBe(SKELETON_HTML);
      expect(root.querySelector("main.gal")!.getAttribute("aria-busy")).toBe("true");
      unmount();
    });
  }

  it("shows only the polite status for a skeleton where the document has no frame", async () => {
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(() => {})));
    const { default: Gallery } = await import("../ui/Gallery.svelte");
    const { mount } = await import("../test/svelte");
    const { root, unmount } = mount(Gallery, {});
    expect(root.querySelector("main.gal > .gskel")!.outerHTML).toBe(`<div class="gskel"><p class="vh" role="status">${LOADING}</p></div>`);
    unmount();
  });
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
