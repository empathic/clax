import { afterEach, describe, expect, it } from "vitest";
import { DEADLINE_MS, MAX_BYTES, MAX_ELEMENTS, serializeSnapshot } from "./snapshot";

const SVG = "http://www.w3.org/2000/svg";

afterEach(() => document.querySelectorAll("iframe").forEach(f => f.remove()));

/** A page in its own frame: jsdom builds the CSSOM only for documents that have a browsing context. */
function doc(body: string, head = ""): Document {
  const f = document.createElement("iframe");
  document.body.append(f);
  const d = f.contentDocument!;
  d.title = "Page";
  d.head.insertAdjacentHTML("beforeend", head);
  d.body.innerHTML = body;
  Object.defineProperty(d, "baseURI", { value: "http://localhost:5173/app/" });
  return d;
}
const snap = (d: Document) => {
  const r = serializeSnapshot(d);
  expect(r.error).toBeNull();
  return r.html;
};
/** The snapshot parsed again, as the daemon's page would be. */
const reparse = (html: string) => new DOMParser().parseFromString(html, "text/html");
/** Every attribute of every element of a parsed snapshot, shadow roots included. */
function allAttrs(d: Document | DocumentFragment): Attr[] {
  const out: Attr[] = [];
  for (const el of d.querySelectorAll("*")) {
    out.push(...el.attributes);
    if (el instanceof HTMLTemplateElement) out.push(...allAttrs(el.content));
  }
  return out;
}

describe("serializeSnapshot", () => {
  it("drops scripts, handlers, comments and dangerous URLs", () => {
    const html = snap(doc(
      `<script>alert(1)</script><noscript>x</noscript><!-- secret --><button onclick="steal()" onmouseover="x()">Save</button>
       <a href="javascript:alert(1)">j</a><a href="/next">n</a><iframe srcdoc="<script>1</script>"></iframe><img src="x.png" onerror="y()">`,
      `<meta http-equiv="refresh" content="0;url=http://evil"><base href="http://evil/">`));
    expect(html).not.toMatch(/<script|<noscript|onclick|onmouseover|onerror|javascript:|srcdoc|secret|http-equiv|<base/i);
    expect(html).toContain(">Save</button>");
    expect(html).toContain('href="http://localhost:5173/next"');
    expect(html).toContain('src="http://localhost:5173/app/x.png"');
    expect(html).toContain('data-clax-placeholder="iframe"');
  });

  it("keeps no form values", () => {
    const d = doc(`<form action="/login"><input type="password" name="pw" value="hunter2"><input type="hidden" name="csrf" value="tok">
      <input name="q" value="typed"><textarea>draft text</textarea><select><option>a</option><option selected>b</option></select></form>`);
    (d.querySelector("input[name=q]") as HTMLInputElement).value = "typed later";
    const html = snap(d);
    expect(html).not.toMatch(/hunter2|tok|typed|draft text|csrf|action=/);
    expect(html).toMatch(/<option selected="">b<\/option>/);
  });

  it("makes URLs absolute in srcset and styles", () => {
    const html = snap(doc(`<img srcset="a.png 1x, /b.png 2x"><div style="background:url('bg.png')"></div>`));
    expect(html).toContain('srcset="http://localhost:5173/app/a.png 1x, http://localhost:5173/b.png 2x"');
    expect(html).toContain('url(&quot;http://localhost:5173/app/bg.png&quot;)');
  });

  it("inlines the CSSOM's rules, including inserted ones", () => {
    const d = doc(`<p>x</p>`, `<style>p { color: red }</style>`);
    (d.querySelector("style") as HTMLStyleElement).sheet!.insertRule("p { font-weight: 700 }", 1);
    const html = snap(d);
    expect(html).toMatch(/<style>p \{\s*color: red;?\s*\}\s*p \{\s*font-weight: 700;?\s*\}<\/style>/);
  });

  it("keeps an unreadable stylesheet as a link to its absolute URL", () => {
    const d = doc(`<p>x</p>`, `<link rel="stylesheet" href="https://cdn.example/x.css"><link rel="preload" href="y.js">`);
    const link = d.querySelector("link") as HTMLLinkElement;
    Object.defineProperty(link, "sheet", { value: { get cssRules() { throw new DOMException("cross-origin", "SecurityError"); }, href: "https://cdn.example/x.css" } });
    const html = snap(d);
    expect(html).toContain('<link rel="stylesheet" href="https://cdn.example/x.css">');
    expect(html).not.toContain("preload");
  });

  it("writes open shadow roots as declarative shadow DOM and skips Clax's own elements", () => {
    const d = doc(`<my-card></my-card><clax-overlay></clax-overlay>`);
    d.querySelector("my-card")!.attachShadow({ mode: "open" }).innerHTML = "<b>inside</b>";
    const html = snap(d);
    expect(html).toContain('<my-card><template shadowrootmode="open"><b>inside</b></template></my-card>');
    expect(html).not.toContain("clax-overlay");
  });

  it("escapes text and attributes", () => {
    const d = doc(`<p title='a"b'>1 &lt; 2 &amp; &lt;/style&gt;</p>`);
    expect(snap(d)).toContain('<p title="a&quot;b">1 &lt; 2 &amp; &lt;/style&gt;</p>');
  });

  it("gives a placeholder page past a cap", () => {
    const d = doc(`${"<i></i>".repeat(50)}`);
    d.title = "<Big>";
    const r = serializeSnapshot(d, { maxElements: 10 });
    expect(r.error).toBe("too_large");
    expect(r.html).toContain("&lt;Big&gt;");
    expect(r.html).toContain("Snapshot unavailable");
    let t = 0;
    expect(serializeSnapshot(doc("<i></i><i></i>"), { deadlineMs: 5, now: () => (t += 10) }).error).toBe("too_large");
  });

  it("uses the spec's caps by default", () => {
    expect([MAX_ELEMENTS, MAX_BYTES, DEADLINE_MS]).toEqual([100_000, 8 * 1024 * 1024, 1500]);
  });

  it("starts a full document with a utf-8 charset and adds no base", () => {
    const html = snap(doc(`<p>x</p>`, `<meta charset="windows-1252"><title>T</title>`));
    expect(html).toMatch(/^<!doctype html><html[^>]*><head><meta charset="utf-8">/);
    expect(html).not.toMatch(/windows-1252|<base/);
  });

  describe("hostile pages", () => {
    it("drops every on* attribute whatever its case or namespace", () => {
      const d = doc(`<div ONCLICK="a()" OnMouseOver="b()" onfocusin="c()">x</div><svg onload="d()"><circle onclick="e()"/></svg>`);
      d.querySelector("div")!.setAttributeNS("urn:x", "x:onclick", "f()");
      const html = snap(d);
      expect(html).not.toMatch(/on(click|mouseover|focusin|load)/i);
      expect(allAttrs(reparse(html)).filter(a => /^on/i.test(a.localName))).toEqual([]);
    });

    it("drops javascript:, vbscript:, data: (other than images) and blob: URLs from every URL attribute", () => {
      const html = snap(doc(
        `<a href=" JaVaScRiPt:alert(1)">a</a><a href="vbscript:msgbox">b</a><a href="data:text/html,<script>1</script>">c</a>
         <img src="blob:http://localhost:5173/u"><img src="data:image/png;base64,AAAA"><video poster="javascript:x"></video>
         <table background="javascript:y"></table><form><button formaction="javascript:z">go</button></form>
         <svg><a xlink:href="javascript:w"><text>t</text></a><image href="javascript:v"/></svg><a ping="http://evil/p" href="#top">t</a>`));
      expect(html).not.toMatch(/javascript|vbscript|data:text|blob:|formaction|ping=/i);
      expect(html).toContain('src="data:image/png;base64,AAAA"');
      expect(html).toContain('href="#top"');
    });

    it("drops dangerous URLs from srcset, keeping commas inside data image URLs", () => {
      const html = snap(doc(`<img srcset="javascript:a 1x, data:image/png;base64,AA,BB 2x, c.png 3x">`));
      expect(html).toContain('srcset="data:image/png;base64,AA,BB 2x, http://localhost:5173/app/c.png 3x"');
    });

    it("neutralizes url() in inline styles and stylesheets", () => {
      const d = doc(`<div style="background-image:url(javascript:alert(1))"></div><svg><rect style="fill:url(#grad)"/></svg>`,
        `<style>div { background: url("vbscript:x") } p { background: url(p.png) }</style>`);
      const html = snap(d);
      expect(html).not.toMatch(/javascript|vbscript/i);
      expect(html).toContain("http://localhost:5173/app/p.png");
      expect(html).toContain("url(&quot;#grad&quot;)");
    });

    it("removes embedded content, base, refresh and non-stylesheet links", () => {
      const html = snap(doc(
        `<object data="x.swf"><param name="a" value="b"></object><embed src="y.swf"><frameset></frameset><canvas></canvas><audio src="a.mp3"></audio>
         <template><img src=x onerror=1></template><noembed>n</noembed><noframes>f</noframes>`,
        `<base href="http://evil/"><meta http-equiv="Refresh" content="0;url=javascript:1"><meta HTTP-EQUIV="set-cookie" content="a=b">
         <link rel="import" href="i.html"><link rel="modulepreload" href="m.js"><link rel="preload" as="script" href="s.js"><link rel="icon" href="f.ico">`));
      for (const t of ["object", "embed", "canvas", "audio"]) expect(html).toContain(`data-clax-placeholder="${t}"`);
      expect(html).not.toMatch(/x\.swf|y\.swf|<param|<template|onerror|noembed|noframes|<base|http-equiv|refresh|import|preload|i\.html|m\.js|s\.js|f\.ico/i);
    });

    it("cleans SVG: scripts, foreignObject handlers and animations of href", () => {
      const html = snap(doc(
        `<svg><script>alert(1)</script><foreignObject><div onclick="x()">fo</div></foreignObject>
         <a href="#a"><set attributeName="href" to="javascript:alert(1)"/><animate attributeName="xlink:href" values="javascript:alert(2)"/>
         <animate attributeName="opacity" values="0;1"/><text>t</text></a></svg>`));
      expect(html).not.toMatch(/<script|onclick|javascript|<set/i);
      expect(html).toContain("<foreignObject><div>fo</div></foreignObject>");
      expect(html).toMatch(/<animate attributeName="opacity"/);
    });

    it("keeps no value of any form control, and reflects choices without values", () => {
      const d = doc(`<input type="text" value="secret-a"><input type="email"><input type="checkbox" name="c" value="secret-b"><input type="radio" checked>
        <button value="secret-c">b</button><select><option value="secret-d">o</option></select><textarea>secret-e</textarea>`);
      (d.querySelector("input[type=email]") as HTMLInputElement).value = "secret-f";
      (d.querySelector("input[type=checkbox]") as HTMLInputElement).checked = true;
      (d.querySelector("input[type=radio]") as HTMLInputElement).checked = false;
      (d.querySelector("textarea") as HTMLTextAreaElement).value = "secret-g";
      const html = snap(d);
      expect(html).not.toMatch(/secret|value=/);
      expect(html).toContain('<input type="checkbox" name="c" checked="">');
      expect(html).toContain('<input type="radio">');
      expect(html).toContain("<textarea></textarea>");
    });

    it("drops comments, including conditional ones, and processing instructions", () => {
      const html = snap(doc(`<!--[if IE]><script>alert(1)</script><![endif]--><p>a<!-- <img src=x onerror=1> -->b</p>`));
      expect(html).toContain("<p>ab</p>");
      expect(html).not.toMatch(/<!--|if IE|onerror/);
    });

    it("omits closed shadow roots and cleans open ones by the same rules", () => {
      const d = doc(`<x-open></x-open><x-closed></x-closed>`);
      d.querySelector("x-open")!.attachShadow({ mode: "open" }).innerHTML = `<script>1</script><b onclick="x()">in</b>`;
      d.querySelector("x-closed")!.attachShadow({ mode: "closed" }).innerHTML = `<b>private</b>`;
      const html = snap(d);
      expect(html).toContain('<x-open><template shadowrootmode="open"><b>in</b></template></x-open>');
      expect(html).toContain("<x-closed></x-closed>");
      expect(html).not.toMatch(/private|<script|onclick/);
    });

    it("skips the elements it is told to", () => {
      const d = doc(`<div id="keep">k</div><div id="mine">m</div>`);
      expect(serializeSnapshot(d, { skip: [d.getElementById("mine")!] }).html).not.toContain("mine");
    });

    it("cannot be broken out of a style element, in HTML or in SVG", () => {
      const d = doc(`<svg><style></style></svg>`, `<style></style>`);
      const css = `p::before { content: "</style><img src=x onerror=alert(1)>" }`;
      d.head.querySelector("style")!.textContent = css;
      d.querySelector("svg style")!.textContent = css;
      const html = snap(d);
      const back = reparse(html);
      expect(back.querySelectorAll("img").length).toBe(0);
      expect(allAttrs(back).filter(a => /^on/i.test(a.name))).toEqual([]);
    });

    it("lowercases nothing into a script: names that only look harmless are judged by their lowercase form", () => {
      const d = doc(`<p></p>`);
      const p = d.querySelector("p")!;
      const s = d.createElementNS("http://www.w3.org/1999/xhtml", "SCRIPT");
      s.textContent = "alert(1)";
      p.append(s, d.createElementNS(SVG, "Script"), d.createElementNS("http://www.w3.org/1999/xhtml", "IFRAME"));
      const html = snap(d);
      expect(html).not.toMatch(/<script|alert/i);
      expect(reparse(html).querySelectorAll("script, iframe").length).toBe(0);
    });

    it("drops elements and attributes whose names would not survive a parse", () => {
      // Chrome accepts names jsdom refuses (any character but whitespace, NUL, "/" and ">"); give them as the DOM would report them.
      const d = doc(`<p><span>s</span></p>`);
      const p = d.querySelector("p")!;
      Object.defineProperty(p, "attributes", { value: [{ name: 'x"onclick', localName: 'x"onclick', value: "1" }, { name: "title", localName: "title", value: "t" }] });
      Object.defineProperty(d.querySelector("span")!, "localName", { value: "a<img" });
      const html = snap(d);
      expect(html).toContain('<p title="t"></p>');
      expect(html).not.toMatch(/x"onclick|a<img|>s</);
    });

    it("reports very deep nesting as too large rather than overflowing the stack", () => {
      const d = doc("");
      let at: Element = d.body;
      for (let i = 0; i < 1_100; i++) at = at.appendChild(d.createElement("div"));
      expect(serializeSnapshot(d).error).toBe("too_large");
    });
  });

  describe("caps", () => {
    it("counts bytes of UTF-8, not UTF-16 code units", () => {
      const d = doc(`<p>${"é".repeat(600)}</p>`);
      expect(serializeSnapshot(d, { maxBytes: 1000 }).error).toBe("too_large");
      expect(serializeSnapshot(d, { maxBytes: 2000 }).error).toBeNull();
    });

    it("accepts a page exactly at the element cap and refuses one past it", () => {
      const d = doc(`<i></i>`);
      const n = d.getElementsByTagName("*").length;
      expect(serializeSnapshot(d, { maxElements: n }).error).toBeNull();
      expect(serializeSnapshot(d, { maxElements: n - 1 }).error).toBe("too_large");
    });

    it("measures the deadline on the injected clock only", () => {
      let t = 0;
      const ok = serializeSnapshot(doc("<i></i><i></i>"), { deadlineMs: 1500, now: () => (t += 1) });
      expect(ok.error).toBeNull();
      t = 0;
      const late = serializeSnapshot(doc("<i></i><i></i>"), { deadlineMs: 1500, now: () => (t += 1000) });
      expect(late.error).toBe("too_large");
      expect(late.html).toMatch(/^<!doctype html>/);
    });

    it("keeps the placeholder page's title short and escaped", () => {
      const d = doc("<i></i>");
      d.title = `</title><script>x</script>${"a".repeat(500)}`;
      const r = serializeSnapshot(d, { maxElements: 1 });
      expect(r.html).not.toContain("<script");
      expect(r.html.length).toBeLessThan(1000);
    });
  });
});
