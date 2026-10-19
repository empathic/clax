import { beforeAll, describe, expect, it } from "vitest";
import Markdown from "../ui/Markdown.svelte";
import { flush, mount } from "../test/svelte";
import { type Block, parse, plain, safeHref } from "./markdown";
import { loadMarkdown, md, plainText } from "./md-load.svelte";
import { itemText } from "../q/inbox-model";
import type { InboxItem } from "../api";

beforeAll(async () => { await loadMarkdown(); });

/** `src` rendered by the component, after the module loaded. */
function render(src: string): HTMLElement {
  const m = mount(Markdown, { text: src });
  flush();
  return m.root.querySelector(".md")!;
}

/** What a rendered tree may hold: the elements the renderer makes, no
 * attribute but these, every link to http(s) or mailto. */
const ELEMENTS = new Set(["DIV", "P", "SPAN", "H3", "H4", "H5", "H6", "PRE", "CODE", "BLOCKQUOTE", "UL", "OL", "LI", "INPUT", "TABLE", "THEAD", "TBODY", "TR", "TH", "TD", "HR", "EM", "STRONG", "DEL", "BR", "A"]);
const ATTRIBUTES = new Set(["class", "href", "target", "rel", "start", "type", "disabled", "checked", "style"]);
function assertInert(root: HTMLElement): void {
  for (const el of [root, ...root.querySelectorAll("*")]) {
    expect(ELEMENTS.has(el.tagName), `element ${el.tagName}`).toBe(true);
    for (const a of el.attributes) {
      expect(ATTRIBUTES.has(a.name), `attribute ${a.name} on ${el.tagName}`).toBe(true);
      if (a.name === "style") expect(a.value).toMatch(/^text-align: (left|center|right);$/);
    }
    if (el.tagName === "A") {
      expect(new URL(el.getAttribute("href")!).protocol).toMatch(/^(https?|mailto):$/);
      expect(el.getAttribute("target")).toBe("_blank");
      expect(el.getAttribute("rel")).toBe("noopener noreferrer nofollow");
    }
    if (el.tagName === "INPUT") {
      expect(el.getAttribute("type")).toBe("checkbox");
      expect(el.hasAttribute("disabled")).toBe(true);
    }
  }
}

describe("safeHref", () => {
  it("keeps absolute http, https and mailto addresses, as the URL parser writes them", () => {
    expect(safeHref("https://example.com/a?b=1#c")).toBe("https://example.com/a?b=1#c");
    expect(safeHref("HTTP://Example.COM")).toBe("http://example.com/");
    expect(safeHref("mailto:a@example.com")).toBe("mailto:a@example.com");
  });

  it("refuses every other scheme, however it is spelt, and anything relative", () => {
    for (const h of [
      "javascript:alert(1)", "jAvAsCrIpT:alert(1)", " javascript:alert(1)", "java\tscript:alert(1)", "java\nscript:alert(1)",
      "\u0001javascript:alert(1)", "&#106;avascript:alert(1)", "&#x6A;avascript:alert(1)", "javascript&colon;alert(1)",
      "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==", "DATA:image/svg+xml,<svg onload=alert(1)>", "vbscript:msgbox(1)",
      "file:///etc/passwd", "blob:https://example.com/x", "about:blank", "ftp://example.com", "/relative", "//example.com", "#top", "", "%6Aavascript:alert(1)",
    ]) expect(safeHref(h), h).toBeNull();
  });
});

describe("parse: untrusted text stays text", () => {
  const corpus: [string, string][] = [
    ["a script element", "<script>alert(1)</script>"],
    ["a script in a paragraph", "hello <script>alert(1)</script> world"],
    ["an event handler on raw HTML", "<div onclick=\"alert(1)\">x</div>"],
    ["an inline event handler", "a <b onmouseover=alert(1)>b</b> c"],
    ["a raw image", "<img src=x onerror=alert(1)>"],
    ["a raw image inline", "look: <img src=\"https://evil.example/t.png\">"],
    ["an iframe", "<iframe src=\"https://evil.example\"></iframe>"],
    ["an svg", "<svg><script>alert(1)</script></svg>"],
    ["a style element", "<style>body{display:none}</style>"],
    ["a raw link", "<a href=\"javascript:alert(1)\">x</a>"],
    ["a javascript: link", "[click](javascript:alert(1))"],
    ["a mixed-case scheme", "[click](jAvAsCrIpT:alert(1))"],
    ["an entity-encoded scheme", "[click](&#106;avascript:alert(1))"],
    ["a hex entity scheme", "[click](&#x6A;&#x61;vascript:alert(1))"],
    ["a named entity colon", "[click](javascript&colon;alert(1))"],
    ["an escaped scheme", "[click](java\\script:alert(1))"],
    ["an angle-bracket destination", "[click](<javascript:alert(1)>)"],
    ["a data: link", "[click](data:text/html,<script>alert(1)</script>)"],
    ["a vbscript: link", "[click](vbscript:msgbox(1))"],
    ["a reference link", "[click][x]\n\n[x]: javascript:alert(1)"],
    ["an autolink", "<javascript:alert(1)>"],
    ["nested brackets", "[[a](javascript:alert(1))](https://example.com)"],
    ["a link in a link", "[a [b](https://evil.example) c](https://example.com)"],
    ["an image", "![alt](https://evil.example/t.png)"],
    ["an image with an onerror title", "![x](https://evil.example/t.png \"onerror=alert(1)\")"],
    ["a javascript: image", "![x](javascript:alert(1))"],
    ["a data: image", "![x](data:image/png;base64,AAAA)"],
    ["an image inside a link", "[![alt](https://evil.example/t.png)](https://example.com)"],
    ["entities", "&lt;script&gt;alert(1)&lt;/script&gt; &#60;img src=x onerror=alert(1)&#62;"],
    ["a quote breaking out", "[x](https://example.com/\"onmouseover=\"alert(1))"],
    ["html in a table", "| a |\n|---|\n| <img src=x onerror=alert(1)> |"],
    ["html in a code span", "`<img src=x onerror=alert(1)>`"],
    ["html in a code block", "```html\n<script>alert(1)</script>\n```"],
    ["a deep nest", `${">".repeat(400)} x`],
  ];

  it.each(corpus)("%s makes only the renderer's elements, with safe links", (_, src) => {
    assertInert(render(src));
  });

  it("shows raw HTML as its text", () => {
    expect(render("<script>alert(1)</script>").textContent).toBe("<script>alert(1)</script>");
    expect(render("a <b onmouseover=alert(1)>b</b> c").textContent).toBe("a <b onmouseover=alert(1)>b</b> c");
    expect(render("<img src=x onerror=alert(1)>").textContent).toBe("<img src=x onerror=alert(1)>");
    expect(render("<iframe src=\"https://evil.example\"></iframe>").textContent).toBe("<iframe src=\"https://evil.example\"></iframe>");
  });

  it("drops a link to another scheme, keeping its text", () => {
    for (const src of ["[click](javascript:alert(1))", "[click](jAvAsCrIpT:alert(1))", "[click](&#106;avascript:alert(1))", "[click](data:text/html,x)", "[click](vbscript:x)"]) {
      const el = render(src);
      expect(el.querySelector("a"), src).toBeNull();
      expect(el.textContent, src).toBe("click");
    }
  });

  it("makes an image a link to its address, named by its alt text or else the address, and never an img", () => {
    const a = render("![a chart](https://example.com/c.png)");
    expect(a.querySelector("img")).toBeNull();
    expect(a.querySelector("a")!.getAttribute("href")).toBe("https://example.com/c.png");
    expect(a.querySelector("a")!.textContent).toBe("a chart");
    expect(render("![](https://example.com/c.png)").querySelector("a")!.textContent).toBe("https://example.com/c.png");
    const b = render("![x](javascript:alert(1))");
    expect(b.querySelector("a")).toBeNull();
    expect(b.textContent).toBe("x");
  });

  it("never nests a link in a link", () => {
    const el = render("[a [b](https://evil.example) c](https://example.com)");
    expect(el.querySelectorAll("a a")).toHaveLength(0);
  });

  it("decodes the common named entities and the numeric ones in text, as text", () => {
    const el = render("&lt;b&gt;x&lt;/b&gt; &amp; &#60;i&#62;");
    expect(el.textContent).toBe("<b>x</b> & <i>");
    expect(el.querySelector("b, i")).toBeNull();
  });
});

describe("parse: GitHub-flavored Markdown", () => {
  it("reads emphasis, strong, strikethrough and inline code", () => {
    const el = render("*em* **strong** ~~gone~~ `x < y`");
    expect(el.querySelector("em")!.textContent).toBe("em");
    expect(el.querySelector("strong")!.textContent).toBe("strong");
    expect(el.querySelector("del")!.textContent).toBe("gone");
    expect(el.querySelector("code")!.textContent).toBe("x < y");
  });

  it("keeps a fenced code block's text exactly", () => {
    const el = render("```js\nconst a = \"<b>\";\n  indented();\n```");
    expect(el.querySelector("pre > code")!.textContent).toBe("const a = \"<b>\";\n  indented();");
  });

  it("reads lists, ordered ones from their start, and task lists with read-only boxes", () => {
    const el = render("- one\n- two\n\n3. three\n4. four\n\n- [ ] todo\n- [x] done");
    expect([...el.querySelectorAll("ul")[0].children].map(li => li.textContent)).toEqual(["one", "two"]);
    expect(el.querySelector("ol")!.getAttribute("start")).toBe("3");
    const boxes = [...el.querySelectorAll<HTMLInputElement>("li.task > input")];
    expect(boxes.map(b => [b.checked, b.disabled])).toEqual([[false, true], [true, true]]);
    expect([...el.querySelectorAll("li.task")].map(li => li.textContent)).toEqual(["todo", "done"]);
  });

  it("reads blockquotes, headings set under the page's, and rules", () => {
    const el = render("# Title\n\n## Sub\n\n#### Deep\n\n> quoted\n\n---\n\nafter");
    expect([...el.querySelectorAll("h3, h4, h5, h6")].map(h => [h.tagName, h.className, h.textContent])).toEqual([["H3", "h1", "Title"], ["H4", "h2", "Sub"], ["H6", "h4", "Deep"]]);
    expect(el.querySelector("blockquote")!.textContent).toBe("quoted");
    expect(el.querySelector("hr")).not.toBeNull();
  });

  it("reads tables with their alignment", () => {
    const el = render("| a | b | c |\n|:--|:-:|--:|\n| 1 | **2** | 3 |");
    expect([...el.querySelectorAll("th")].map(t => t.textContent)).toEqual(["a", "b", "c"]);
    expect([...el.querySelectorAll("td")].map(t => (t as HTMLElement).style.textAlign)).toEqual(["left", "center", "right"]);
    expect(el.querySelector("td strong")!.textContent).toBe("2");
  });

  it("links bare URLs, www. addresses and email addresses, opening in a new tab", () => {
    const el = render("see https://example.com/x, www.example.org and a@example.com");
    expect([...el.querySelectorAll("a")].map(a => a.getAttribute("href"))).toEqual(["https://example.com/x", "http://www.example.org/", "mailto:a@example.com"]);
    assertInert(el);
  });

  it("keeps a plain comment's text and line breaks as typed", () => {
    const el = render("Button looks off\nsecond line");
    expect(el.querySelectorAll("p")).toHaveLength(1);
    expect(el.textContent).toBe("Button looks off\nsecond line");
  });

  it("is a paragraph per block of text", () => {
    expect(parse("one\n\ntwo")).toEqual([{ t: "p", c: [{ t: "text", v: "one" }] }, { t: "p", c: [{ t: "text", v: "two" }] }] satisfies Block[]);
    expect(parse("")).toEqual([]);
  });
});

describe("before the module loads", () => {
  it("shows the text as typed", () => {
    const m = md.m;
    md.m = null;
    try {
      const el = mount(Markdown, { text: "**not yet** <b>x</b>" }).root.querySelector(".md")!;
      expect(el.querySelector("p")!.textContent).toBe("**not yet** <b>x</b>");
      expect(el.querySelector("strong, b")).toBeNull();
      expect(plainText("**x**")).toBe("**x**");
    } finally { md.m = m; }
  });
});

describe("plain", () => {
  it("takes the syntax away, a line per block", () => {
    expect(plain("# Title\n\nSome **bold** and `code`, a [link](https://example.com) and ![a chart](https://example.com/c.png).\n\n- [x] done\n- todo\n\n> quoted\n\n```\nconst a = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |"))
      .toBe("Title\nSome bold and code, a link and a chart.\ndone\ntodo\nquoted\nconst a = 1;\na b\n1 2");
  });

  it("keeps raw HTML as its text", () => {
    expect(plain("hi <img src=x onerror=alert(1)>")).toBe("hi <img src=x onerror=alert(1)>");
  });

  it("is what the inbox's one-line text shows for a reply", () => {
    const item = { id: "i", seq: 1, kind: "reply", reply: { body: "**Fixed** in `v4`:\n\n- the [header](https://example.com)" } } as unknown as InboxItem;
    expect(itemText(item)).toBe("Fixed in v4: the header");
  });
});
