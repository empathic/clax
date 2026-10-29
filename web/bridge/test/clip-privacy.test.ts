import { afterEach, describe, expect, it, vi } from "vitest";

// The real modern-screenshot clones the page and builds the SVG it would
// draw, with the clip's hooks; only the drawing into a PNG (a canvas, which
// jsdom lacks) is replaced. Each render's SVG is kept as markup to read.
const svgs: string[] = [];
vi.mock("modern-screenshot", async importOriginal => {
  const real = await importOriginal<typeof import("modern-screenshot")>();
  return {
    ...real,
    createContext: (el: Node, opts: object) => real.createContext(el, { ...opts, font: false, autoDestruct: false }),
    domToPng: async (ctx: Parameters<typeof real.domToForeignObjectSvg>[0]) => {
      const svg = await real.domToForeignObjectSvg(ctx);
      svgs.push(new XMLSerializer().serializeToString(svg));
      return "data:image/png;base64,iVBORw==";
    },
  };
});

// jsdom has no canvas: one reads as a blank one does in a browser.
HTMLCanvasElement.prototype.toDataURL = () => "data:,";

const { renderClipWithReport, renderViewportClip } = await import("../src/clip");

const sized = (el: Element, left: number, top: number, width: number, height: number) => {
  (el as HTMLElement).getBoundingClientRect = () => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON() {} }) as DOMRect;
};

/** The last render's copy of the page, parsed back. */
function rendered(): Document {
  return new DOMParser().parseFromString(svgs.at(-1)!, "image/svg+xml");
}
const inCopy = (sel: string) => rendered().querySelector(sel);

async function render(html: string, setup: (root: HTMLElement) => void = () => {}) {
  document.body.innerHTML = `<div id="root">${html}</div>`;
  const root = document.getElementById("root")!;
  sized(root, 10, 20, 400, 300);
  setup(root);
  return renderClipWithReport(root, window, 20_000);
}

afterEach(() => { svgs.length = 0; document.body.innerHTML = ""; vi.restoreAllMocks(); });

describe("viewport renders", () => {
  it("draw fixed and stuck elements where the viewport shows them", async () => {
    document.body.innerHTML = `<header id="bar" style="position: fixed; top: 0">Menu</header><main><h2 id="stuck" style="position: sticky; top: 0">Section</h2><p>text</p></main>`;
    vi.spyOn(window, "scrollY", "get").mockReturnValue(300);
    sized(document.documentElement, 0, -300, 1024, 3000);
    sized(document.getElementById("bar")!, 0, 0, 1024, 50);
    // Stuck at the viewport's top; unstuck it would sit 120 px lower.
    const h2 = document.getElementById("stuck")!;
    h2.getBoundingClientRect = () => {
      const top = h2.style.position === "static" ? 120 : 0;
      return { left: 0, top, width: 1024, height: 30, right: 1024, bottom: top + 30, x: 0, y: top, toJSON() {} } as DOMRect;
    };
    await renderViewportClip(window, 20_000);
    // The page's own element is put back as it was.
    expect(h2.getAttribute("style")).toBe("position: sticky; top: 0px;");
    const bar = inCopy("#bar")!.getAttribute("style")!;
    expect(bar).toMatch(/top: 300px !important/);
    expect(bar).toMatch(/left: 0px !important/);
    const stuck = inCopy("#stuck")!.getAttribute("style")!;
    expect(stuck).toMatch(/position: relative !important/);
    expect(stuck).toMatch(/top: -120px !important/);
  });
});

describe("private DOM renders", () => {
  it("blanks typed input values", async () => {
    await render(`<input id="q" value="draft"><textarea id="t">old</textarea><select id="s"><option>Alpha</option><option>Beta</option></select><div id="e" contenteditable>my note</div><input id="ok" type="submit" value="Send">`, () => {
      (document.getElementById("q") as HTMLInputElement).value = "a private search";
      (document.getElementById("t") as HTMLTextAreaElement).value = "an unsent comment";
      (document.getElementById("s") as HTMLSelectElement).selectedIndex = 1;
    });
    const svg = svgs.at(-1)!;
    for (const secret of ["a private search", "draft", "an unsent comment", "old", "Beta", "Alpha", "my note"]) expect(svg).not.toContain(secret);
    expect(inCopy("#q")!.getAttribute("value") ?? "").toBe("");
    expect(inCopy("#t")!.textContent).toBe("");
    expect(inCopy("#e")!.textContent).toBe("");
    // A button's value is its label, not something typed.
    expect(inCopy("#ok")!.getAttribute("value")).toBe("Send");
  });

  it("keeps data-clax-show values", async () => {
    await render(`<input id="q" data-clax-show><form data-clax-show><textarea id="t"></textarea><select id="s"><option>Alpha</option><option>Beta</option></select></form>`, () => {
      (document.getElementById("q") as HTMLInputElement).value = "shown search";
      (document.getElementById("t") as HTMLTextAreaElement).value = "shown text";
      (document.getElementById("s") as HTMLSelectElement).selectedIndex = 1;
    });
    // What draws: an input's value attribute, a textarea's text, the
    // selected option.
    expect(inCopy("#q")!.getAttribute("value")).toBe("shown search");
    expect(inCopy("#t")!.textContent).toBe("shown text");
    const options = Array.from(inCopy("#s")!.querySelectorAll("option"));
    expect(options.map(o => [o.textContent, o.hasAttribute("selected")])).toEqual([["Alpha", false], ["Beta", true]]);
  });

  it("blanks a field in an open shadow root and in a same-origin frame", async () => {
    await render(`<div id="host"></div><iframe id="f"></iframe>`, () => {
      const shadow = document.getElementById("host")!.attachShadow({ mode: "open" });
      shadow.innerHTML = `<input id="sq">`;
      (shadow.getElementById("sq") as HTMLInputElement).value = "shadow secret";
      const inner = (document.getElementById("f") as HTMLIFrameElement).contentDocument!;
      inner.body.innerHTML = `<input id="fq">`;
      (inner.getElementById("fq") as HTMLInputElement).value = "frame secret";
    });
    const svg = svgs.at(-1)!;
    expect(svg).not.toContain("shadow secret");
    expect(svg).not.toContain("frame secret");
  });

  it("keeps the text of a contenteditable=\"false\" part of an editable region", async () => {
    await render(`<div id="e" contenteditable>typed words<span id="chip" contenteditable="false">Assignee: Ada</span></div>`);
    expect(svgs.at(-1)).not.toContain("typed words");
    expect(inCopy("#chip")!.textContent).toBe("Assignee: Ada");
  });

  it("covers a redaction inside a shadow root", async () => {
    await render(`<div id="host"></div>`, () => {
      const shadow = document.getElementById("host")!.attachShadow({ mode: "open" });
      shadow.innerHTML = `<p id="sp" data-clax-redact>shadow card</p><canvas id="sc" data-clax-redact></canvas>`;
      sized(shadow.getElementById("sc")!, 60, 80, 30, 20);
    });
    const doc = rendered();
    expect(doc.querySelector("#sp")!.hasAttribute("data-clax-redact")).toBe(true);
    const block = doc.querySelector("rect[data-clax-redacted]")!;
    expect([block.getAttribute("x"), block.getAttribute("y"), block.getAttribute("width"), block.getAttribute("height")]).toEqual(["50", "60", "30", "20"]);
  });

  it("keeps the values under a rendered root that is itself data-clax-show", async () => {
    document.body.innerHTML = `<section data-clax-show><div id="root"><input id="q"></div></section>`;
    const root = document.getElementById("root")!;
    sized(root, 0, 0, 200, 100);
    (document.getElementById("q") as HTMLInputElement).value = "shown";
    await renderClipWithReport(root, window, 20_000);
    expect(inCopy("#q")!.getAttribute("value")).toBe("shown");
  });

  it("always blanks passwords", async () => {
    await render(`<form data-clax-show><input id="p" type="password" data-clax-show value="hunter2"></form>`, () => {
      (document.getElementById("p") as HTMLInputElement).value = "correct horse";
    });
    expect(svgs.at(-1)).not.toContain("correct horse");
    expect(svgs.at(-1)).not.toContain("hunter2");
  });

  it("draws data-clax-redact as a block", async () => {
    await render(`<p id="card" data-clax-redact>4111 1111 1111 1111 <img id="i" src="data:image/png;base64,iVBORw=="></p><canvas id="c" data-clax-redact></canvas>`, root => {
      sized(document.getElementById("c")!, 30, 60, 50, 40);
      void root;
    });
    const doc = rendered();
    const css = Array.from(doc.querySelectorAll("style")).map(s => s.textContent).join("\n");
    // The redacted element paints its own box in the block colour (black on
    // this light page) and nothing of what it holds.
    expect(css).toMatch(/\[data-clax-redact\]\s*\{[^}]*background:\s*#000\s*!important/);
    expect(css).toMatch(/\[data-clax-redact\] \*[^{]*\{[^}]*visibility:\s*hidden\s*!important/);
    // A redacted canvas is drawn from a copy that keeps none of its
    // attributes, so its box is covered by a block over the picture, at its
    // place in the picture (CSS px from the picture's top left corner).
    const block = doc.querySelector("rect[data-clax-redacted]")!;
    expect(block).not.toBeNull();
    expect([block.getAttribute("x"), block.getAttribute("y"), block.getAttribute("width"), block.getAttribute("height"), block.getAttribute("fill")]).toEqual(["20", "40", "50", "40", "#000"]);
  });

  it("draws the redaction block white on a dark page", async () => {
    await render(`<p data-clax-redact>secret</p>`, root => { root.style.backgroundColor = "rgb(18, 18, 18)"; });
    const css = Array.from(rendered().querySelectorAll("style")).map(s => s.textContent).join("\n");
    expect(css).toMatch(/\[data-clax-redact\]\s*\{[^}]*background:\s*#fff\s*!important/);
  });

  it("drops data-clax-ignore and the overlay host", async () => {
    await render(`<p id="kept">page</p><div id="chat" data-clax-ignore><p>support chat</p></div><clax-overlay id="ov"></clax-overlay>`);
    expect(inCopy("#kept")).not.toBeNull();
    expect(inCopy("#chat")).toBeNull();
    expect(inCopy("#ov")).toBeNull();
    expect(svgs.at(-1)).not.toContain("support chat");
  });

  it("lists a cross-origin iframe as omitted", async () => {
    const res = await render(`<iframe id="f" src="https://other.example/embed"></iframe><iframe id="same" srcdoc="<p>here</p>"></iframe><img id="far" src="https://cdn.example/a.png"><div data-clax-ignore><iframe id="hidden" src="https://other.example/x"></iframe></div>`, () => {
      sized(document.getElementById("f")!, 50, 70, 300, 150);
      sized(document.getElementById("same")!, 50, 230, 100, 50);
      sized(document.getElementById("far")!, 380, 280, 100, 100);
      sized(document.getElementById("hidden")!, 0, 0, 100, 100);
    });
    // In the image's pixels from its top left corner (the root at 10,20 is
    // drawn at one pixel per CSS px), cut to the image.
    expect(res.omitted).toEqual([
      { kind: "iframe", rect: { x: 40, y: 50, w: 300, h: 150 } },
      { kind: "image", rect: { x: 370, y: 260, w: 30, h: 40 } },
    ]);
    expect([res.at, res.scale]).toEqual([{ x: 10, y: 20, w: 400, h: 300 }, 1]);
  });

  it("places omissions in the image of a cropped clip drawn at twice the scale", async () => {
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
    document.body.innerHTML = `<div id="root"><iframe id="f" src="https://other.example/embed"></iframe></div>`;
    const root = document.getElementById("root")!;
    sized(root, 10, 20, 400, 300);
    sized(document.getElementById("f")!, 150, 90, 300, 150);
    // The crop shows 110,70 (200 x 100) of the viewport.
    const res = await renderClipWithReport(root, window, 20_000, { crop: { x: 100, y: 50, w: 200, h: 100 } });
    expect(res.scale).toBe(2);
    expect(res.omitted).toEqual([{ kind: "iframe", rect: { x: 80, y: 40, w: 320, h: 160 } }]);
  });
});
