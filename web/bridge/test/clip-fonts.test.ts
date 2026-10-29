import { afterEach, describe, expect, it, vi } from "vitest";

// modern-screenshot is replaced: each render records whether it was asked to
// read web fonts, and answers a tiny PNG.
const fonts: unknown[] = [];
vi.mock("modern-screenshot", () => ({
  createContext: async (_el: Element, opts: { font?: unknown }) => { fonts.push(opts.font); return { opts }; },
  domToPng: async () => "data:image/png;base64,iVBORw==",
  destroyContext: () => {},
}));

const { metaRefusesInlineStyle, renderClipWithReport, withoutWebFonts } = await import("../src/clip");

const sized = (el: Element) => {
  (el as HTMLElement).getBoundingClientRect = () => ({ left: 0, top: 0, width: 50, height: 20, right: 50, bottom: 20, x: 0, y: 0, toJSON() {} }) as DOMRect;
};

afterEach(() => { fonts.length = 0; document.head.innerHTML = ""; });

describe("web fonts in clips", () => {
  it("are read unless the host says the page's policy refuses inline style, and then never first tried", async () => {
    const doc = document.implementation.createHTMLDocument("strict");
    const el = doc.createElement("p");
    el.textContent = "Total";
    doc.body.append(el);
    sized(el);
    await renderClipWithReport(el, window);
    expect(fonts).toEqual([undefined]);
    withoutWebFonts(doc);
    await renderClipWithReport(el, window);
    expect(fonts).toEqual([undefined, false]);
  });

  it("reads a policy the page states in a meta element", () => {
    const meta = (content: string) => { document.head.innerHTML = ""; const m = document.createElement("meta"); m.httpEquiv = "Content-Security-Policy"; m.content = content; document.head.append(m); };
    expect(metaRefusesInlineStyle(document)).toBe(false);
    meta("default-src 'self'; style-src 'self' 'unsafe-inline'");
    expect(metaRefusesInlineStyle(document)).toBe(false);
    meta("default-src 'self'");
    expect(metaRefusesInlineStyle(document)).toBe(true);
    meta("style-src 'self' 'unsafe-inline' 'nonce-abc'");
    expect(metaRefusesInlineStyle(document)).toBe(true);
    meta("script-src 'self'");
    expect(metaRefusesInlineStyle(document)).toBe(false);
  });
});
