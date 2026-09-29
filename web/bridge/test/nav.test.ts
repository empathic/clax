import { afterEach, describe, expect, it } from "vitest";
import { type PlaceLocation, followInPlace, linkToHandOver, linkedPage } from "../src/nav";

const SUB = "http://7q3k9mzx2b4t.localhost:7480/v/2/";
const MAIN = "http://localhost:7480/c/7q3k9mzx2b4t/v/2/";

describe("linkedPage", () => {
  it("names the version's page a link leads to, with its fragment, from any page, in both frame modes", () => {
    expect(linkedPage("about.html", SUB, "index.html")).toEqual({ file: "about.html", hash: "" });
    expect(linkedPage("about.html#team", MAIN, "index.html")).toEqual({ file: "about.html", hash: "#team" });
    expect(linkedPage("index.html", `${SUB}about.html`, "about.html")).toEqual({ file: "index.html", hash: "" });
    expect(linkedPage("./#top", `${MAIN}about.html`, "about.html")).toEqual({ file: "index.html", hash: "#top" });
    expect(linkedPage("../a%20b.html", `${SUB}docs/x.html`, "docs/x.html")).toEqual({ file: "a b.html", hash: "" });
    expect(linkedPage("docs/x.html", MAIN, "index.html")).toEqual({ file: "docs/x.html", hash: "" });
  });

  it("leaves links to the same page, in any spelling, to the browser", () => {
    expect(linkedPage("#top", SUB, "index.html")).toBeNull();
    expect(linkedPage("", `${SUB}about.html`, "about.html")).toBeNull();
    expect(linkedPage("index.html", SUB, "index.html")).toBeNull();
    expect(linkedPage("index.html#top", MAIN, "index.html")).toBeNull();
    expect(linkedPage("./", SUB, "index.html")).toBeNull();
    expect(linkedPage("about.html#x", `${SUB}about.html`, "about.html")).toBeNull();
  });

  it("leaves other links alone", () => {
    expect(linkedPage("about.html?x=1", SUB, "index.html"), "a query the shell URL cannot carry").toBeNull();
    expect(linkedPage("https://example.com/", SUB, "index.html"), "another site").toBeNull();
    expect(linkedPage("/v/1/about.html", SUB, "index.html"), "another version").toBeNull();
    expect(linkedPage("/c/9zzzzzzzzzzz/v/2/a.html", MAIN, "index.html"), "another artifact").toBeNull();
    for (const href of ["mailto:x@y.z", "javascript:void(0)", "data:text/html,<p>x</p>", `blob:${SUB}0f7c`]) {
      expect(linkedPage(href, SUB, "index.html"), href).toBeNull();
    }
  });
});

describe("linkToHandOver", () => {
  afterEach(() => { document.head.replaceChildren(); document.body.replaceChildren(); });

  /** Clicks the first link in `html` with `init`; returns what the bridge would hand over. */
  function click(html: string, init: MouseEventInit = {}, opts: { welcomed?: boolean; cancel?: boolean } = {}) {
    document.body.innerHTML = html;
    const a = document.querySelector("a")!;
    if (opts.cancel) a.addEventListener("click", e => e.preventDefault());
    let out: ReturnType<typeof linkToHandOver> | undefined;
    const listen = (e: Event) => { out = linkToHandOver(e as MouseEvent, { welcomed: opts.welcomed ?? true, pageUrl: SUB, file: "index.html" }); e.preventDefault(); };
    document.addEventListener("click", listen);
    a.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, ...init }));
    document.removeEventListener("click", listen);
    return out;
  }
  const link = `<a href="${SUB}about.html#team"><span>About</span></a>`;

  it("hands over a plain click on a link to another page, from a child of the link too", () => {
    expect(click(link)).toEqual({ kind: "page", file: "about.html", hash: "#team" });
  });

  it("leaves modified clicks, other buttons, cancelled clicks, and a bridge not yet welcomed alone", () => {
    for (const init of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }]) {
      expect(click(link, init), JSON.stringify(init)).toBeNull();
    }
    expect(click(link, {}, { cancel: true }), "cancelled by the page").toBeNull();
    expect(click(link, {}, { welcomed: false }), "not welcomed").toBeNull();
  });

  it("follows this page under another spelling in place, and leaves its own path to the browser", () => {
    expect(click(`<a href="${SUB}index.html">Home</a>`)).toEqual({ kind: "self", hash: "" });
    expect(click(`<a href="${SUB}index.html#top">Top</a>`)).toEqual({ kind: "self", hash: "#top" });
    expect(click(`<a href="${SUB}#top">Top</a>`)).toBeNull();
    expect(click(`<a href="${SUB}">Home</a>`)).toBeNull();
  });

  it("leaves downloads, other targets, external links, and pages with another base target alone", () => {
    expect(click(`<a download href="${SUB}about.html">x</a>`)).toBeNull();
    expect(click(`<a target="_blank" href="${SUB}about.html">x</a>`)).toBeNull();
    expect(click(`<a target="_self" href="${SUB}about.html">x</a>`)).toEqual({ kind: "page", file: "about.html", hash: "" });
    expect(click(`<a rel="noopener external" href="${SUB}about.html">x</a>`)).toBeNull();
    document.head.innerHTML = `<base target="_top">`;
    expect(click(`<a href="${SUB}about.html">x</a>`)).toBeNull();
    document.head.innerHTML = `<base target="_self">`;
    expect(click(`<a href="${SUB}about.html">x</a>`)).toEqual({ kind: "page", file: "about.html", hash: "" });
    for (const href of ["javascript:void(0)", "data:text/html,x", `blob:${SUB}0f7c`]) {
      expect(click(`<a href="${href}">x</a>`), href).toBeNull();
    }
  });
});

describe("followInPlace", () => {
  const at = (hash: string) => {
    const calls: string[] = [];
    const loc = {
      pathname: "/v/2/", search: "", hash,
      get href() { return `http://h/v/2/${this.hash}`; },
      set href(v: string) { calls.push(`href ${v}`); },
      replace(v: string | URL) { calls.push(`replace ${v}`); },
    };
    return { calls, loc: new Proxy(loc, { set(t, k, v) { if (k === "hash") calls.push(`hash ${v}`); return Reflect.set(t, k, v); } }) as unknown as PlaceLocation };
  };
  it("moves to a new fragment as a fragment navigation", () => {
    const { calls, loc } = at("#a");
    followInPlace("#b", loc);
    expect(calls).toEqual(["hash #b"]);
  });
  it("scrolls to the current fragment again, which setting the same hash would not", () => {
    const { calls, loc } = at("#team");
    followInPlace("#team", loc);
    expect(calls).toEqual(["href /v/2/#team"]);
  });
  it("loads the page's URL without a fragment in place of this entry, not a reload", () => {
    const { calls, loc } = at("#team");
    followInPlace("", loc);
    expect(calls).toEqual(["replace /v/2/"]);
  });
});
