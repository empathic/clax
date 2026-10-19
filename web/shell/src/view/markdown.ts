// Comment bodies as Markdown: CommonMark with GitHub's extensions (tables,
// strikethrough, task lists, bare URLs made links), with no images and no
// raw HTML.
//
// The text is untrusted. markdown-it only parses it here (`html: false`,
// so raw HTML is never a tag, only text); nothing here makes HTML. `parse`
// turns its tokens into the closed tree below (`Block`, `Inline`), which
// `ui/Markdown.svelte` renders as elements whose strings are always text.
// An image becomes a link to its address (no remote fetch), and a link is
// kept only when its address is absolute `http:`, `https:` or `mailto:`
// (`safeHref`, on top of markdown-it's own refusal of `javascript:`,
// `vbscript:`, `file:` and `data:`); any other link is its text alone. This
// module is loaded on demand (`md-load.svelte.ts`): no entry carries the
// parser.
import MarkdownIt, { type Token } from "markdown-it";

/** Text-level content. A `link`'s `href` has passed `safeHref`; its
 * `addr` is the address as written, when its text does not already say it
 * (plain text shows it after the text; a rendered link shows it on hover). */
export type Inline =
  | { t: "text"; v: string }
  | { t: "code"; v: string }
  | { t: "em" | "strong" | "del"; c: Inline[] }
  | { t: "link"; href: string; addr: string | null; c: Inline[] }
  | { t: "br" };

export type Align = "left" | "center" | "right" | null;

/** One item of a list: a task item (`- [ ]`, `- [x]`) has a read-only box. */
export type Item = { task: boolean; checked: boolean; c: Block[] };

export type Level = 1 | 2 | 3 | 4 | 5 | 6;

/** Block-level content. `line` is a tight list item's text, set in the
 * item without a paragraph of its own; a `pre`'s `info` is what follows its
 * opening fence (a language, or any words), shown with it. */
export type Block =
  | { t: "p" | "line"; c: Inline[] }
  | { t: "h"; level: Level; c: Inline[] }
  | { t: "pre"; v: string; info: string }
  | { t: "quote"; c: Block[] }
  | { t: "list"; ordered: boolean; start: number | null; items: Item[] }
  | { t: "table"; align: Align[]; head: Inline[][]; rows: Inline[][][] }
  | { t: "hr" };

const SCHEMES = new Set(["http:", "https:", "mailto:"]);

/** `raw` as a link address when it is an absolute `http:`, `https:` or
 * `mailto:` URL (as the URL parser reads it, so case, whitespace and
 * control characters cannot smuggle another scheme through), else null.
 * The address returned is the parser's own serialization. */
export function safeHref(raw: string): string | null {
  let u: URL;
  try { u = new URL(raw); } catch { return null; }
  return SCHEMES.has(u.protocol) ? u.href : null;
}

/** The deepest nesting read as Markdown. markdown-it leaves out what a
 * container nests past it, and emphasis nests without limit, so a body
 * that reaches it is shown as its text (`parse`): nothing in a comment is
 * hidden from the people reading it, and the tree the renderer walks
 * recursively stays shallow. */
const NESTING = 20;

/** Thrown by `nest` past `NESTING`. */
class TooDeep extends Error {}

// markdown-it reads emphasis, links and code in time linear in the length
// of its input (CommonMark's delimiter algorithm). Tables are the
// exception: every row is padded to its header's columns, so a 4 KB body
// of a 500-column header and bare `|` lines is 66,000 cells. A body whose
// tables could hold more than `MAX_CELLS` cells, or a row more than
// `MAX_COLUMNS`, is read with tables off (`plainMd`): its table lines show
// as text. Bare URLs and email addresses become links; a bare domain
// (`README.md`) does not. A link reference definition (`[x]: https://…`)
// is not read as one: it would show nothing, and `[//]: # (…)` would hide
// its text from the people reading a comment that agents read whole. It
// shows as its text.
const OPTIONS = { html: false, linkify: true, typographer: false, breaks: false, maxNesting: NESTING } as const;
const md = new MarkdownIt("default", OPTIONS).disable("reference");
const plainMd = new MarkdownIt("default", OPTIONS).disable(["reference", "table"]);

/** The most cells a comment's tables may have, and columns a table may. */
export const MAX_CELLS = 1500;
export const MAX_COLUMNS = 24;

// A table's delimiter row (`|---|:-:|`), possibly in a quote or a list.
const DELIMITER = /^[ \t>*+\-\d.)]*\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/;

/** Whether `src`'s tables could hold more than `MAX_CELLS` cells (each row
 * is padded to its header's columns, and any line may be a row) or more
 * than `MAX_COLUMNS` columns. */
function tooWide(src: string): boolean {
  const lines = src.split("\n");
  let cols = 0;
  for (const l of lines) if (l.includes("|") && l.includes("-") && DELIMITER.test(l)) cols = Math.max(cols, l.replace(/^[^|]*?(?=\|)|\|[ \t]*$/g, "").split("|").filter(c => c.trim()).length);
  return cols > MAX_COLUMNS || cols * lines.length > MAX_CELLS;
}

/** The words of `s`: its runs of letters and digits. */
const words = (s: string): string[] => s.match(/[\p{L}\p{N}]+/gu) ?? [];

/** Whether every word of `s` is among `of`'s. */
const said = (s: string, of: string): boolean => { const w = new Set(words(of)); return words(s).every(x => w.has(x)); };

/** `t`'s attribute `name`, "" when it has none. */
const attr = (t: Token, name: string): string => String(t.attrGet(name) ?? "");

/** A token with the tokens between it and its closing token. */
type Node = { t: Token; c: Node[] };

/** `ts`, a flat stream of opening, closing and whole tokens, as a tree;
 * `TooDeep` past `NESTING`. */
function nest(ts: readonly Token[]): Node[] {
  const root: Node[] = [];
  const stack: Node[][] = [root];
  for (const t of ts) {
    if (t.nesting === -1) { if (stack.length > 1) stack.pop(); continue; }
    const n: Node = { t, c: [] };
    stack[stack.length - 1].push(n);
    if (t.nesting === 1) stack.push(n.c);
    if (stack.length > NESTING) throw new TooDeep();
  }
  return root;
}

function inline(ns: Node[], inLink = false): Inline[] {
  const out: Inline[] = [];
  const text = (v: string) => {
    if (!v) return;
    const last = out.at(-1);
    if (last?.t === "text") last.v += v;
    else out.push({ t: "text", v });
  };
  const put = (xs: Inline[]) => { for (const x of xs) { if (x.t === "text") text(x.v); else out.push(x); } };
  for (const { t, c } of ns) {
    switch (t.type) {
      case "softbreak": text("\n"); break;
      case "hardbreak": out.push({ t: "br" }); break;
      case "code_inline": out.push({ t: "code", v: t.content }); break;
      case "em_open": out.push({ t: "em", c: inline(c, inLink) }); break;
      case "strong_open": out.push({ t: "strong", c: inline(c, inLink) }); break;
      case "s_open": out.push({ t: "del", c: inline(c, inLink) }); break;
      // Every word of a link shows: a link with no text is named by its
      // address; the address of a link that is not made (another scheme,
      // a relative address) follows its text; a title follows the link.
      case "link_open": {
        const kids = inline(c, true);
        const label = inlineText(kids);
        const addr = md.normalizeLinkText(attr(t, "href"));
        const auto = t.markup === "linkify" || t.markup === "autolink";
        const href = inLink ? null : safeHref(attr(t, "href"));
        if (href) out.push({ t: "link", href, addr: auto || said(addr, label) ? null : addr, c: label.trim() ? kids : [{ t: "text", v: addr }] });
        else { put(kids); if (addr && !auto && !said(addr, label)) text(label.trim() ? ` (${addr})` : addr); }
        const title = attr(t, "title");
        if (title) text(` (${title})`);
        break;
      }
      case "image": {
        // Never an image: a link to it, named by its alt text or its address.
        const alt = inlineText(inline(nest(t.children ?? []), true));
        const addr = md.normalizeLinkText(attr(t, "src"));
        const label = alt.trim() ? alt : addr;
        const href = inLink ? null : safeHref(attr(t, "src"));
        if (href) out.push({ t: "link", href, addr: label === addr ? null : addr, c: [{ t: "text", v: label }] });
        else text(label === addr ? label : `${label} (${addr})`);
        const title = attr(t, "title");
        if (title) text(` (${title})`);
        break;
      }
      // Text, escapes and entities (already decoded), and anything else: as text.
      default: text(t.content); put(inline(c, inLink));
    }
  }
  return out;
}

const ALIGN = /text-align:\s*(left|center|right)/;
const align = (t: Token): Align => (ALIGN.exec(attr(t, "style"))?.[1] as Align | undefined) ?? null;
const cells = (row: Node): Node[] => row.c.filter(x => x.t.type === "th_open" || x.t.type === "td_open");
const cellInline = (cell: Node): Inline[] => inline(nest(cell.c[0]?.t.children ?? []));

const TASK = /^\[([ xX])\][ \t]+/;

/** A list item: `[ ]` or `[x]` and a space opening its first line, as
 * written (not `\[ ]` or `&#91; ]`, which are the author's brackets), make
 * it a task. */
function item(n: Node): Item {
  const c = blocks(n.c);
  const first = c[0];
  const source = n.c[0]?.t.type === "paragraph_open" ? n.c[0].c[0]?.t.content ?? "" : "";
  if (TASK.test(source) && first && (first.t === "p" || first.t === "line") && first.c[0]?.t === "text") {
    const head = first.c[0];
    const m = TASK.exec(head.v);
    if (m) {
      head.v = head.v.slice(m[0].length);
      if (!head.v) first.c.shift();
      return { task: true, checked: m[1] !== " ", c };
    }
  }
  return { task: false, checked: false, c };
}

function blocks(ns: Node[]): Block[] {
  const out: Block[] = [];
  for (const { t, c } of ns) {
    switch (t.type) {
      case "paragraph_open": out.push({ t: t.hidden ? "line" : "p", c: inline(nest(c[0]?.t.children ?? [])) }); break;
      case "heading_open": {
        const n = Number(t.tag.slice(1));
        out.push({ t: "h", level: (n >= 1 && n <= 6 ? n : 6) as Level, c: inline(nest(c[0]?.t.children ?? [])) });
        break;
      }
      case "fence": case "code_block": out.push({ t: "pre", v: t.content.replace(/\n$/, ""), info: t.type === "fence" ? t.info.trim() : "" }); break;
      case "blockquote_open": out.push({ t: "quote", c: blocks(c) }); break;
      case "hr": out.push({ t: "hr" }); break;
      case "bullet_list_open": case "ordered_list_open": {
        const ordered = t.type === "ordered_list_open";
        const s = ordered ? Number(attr(t, "start") || "1") : 1;
        out.push({ t: "list", ordered, start: Number.isSafeInteger(s) && s !== 1 ? s : null, items: c.filter(x => x.t.type === "list_item_open").map(item) });
        break;
      }
      case "table_open": {
        const head = c.find(x => x.t.type === "thead_open")?.c.find(x => x.t.type === "tr_open");
        const body = c.find(x => x.t.type === "tbody_open")?.c.filter(x => x.t.type === "tr_open") ?? [];
        out.push({
          t: "table",
          align: head ? cells(head).map(x => align(x.t)) : [],
          head: head ? cells(head).map(cellInline) : [],
          rows: body.map(r => cells(r).map(cellInline)),
        });
        break;
      }
      case "inline": out.push({ t: "p", c: inline(nest(t.children ?? [])) }); break;
      // Anything else: its source, as text.
      default: {
        const v = t.content.replace(/\n+$/, "");
        if (v.trim()) out.push({ t: "p", c: [{ t: "text", v }] });
      }
    }
  }
  return out;
}

// What Markdown spends of a body's words, and only that: character
// references, list markers and task markers.
const MARKUP = /&(?:#\d+|#[xX][\da-fA-F]+|[A-Za-z][A-Za-z\d]*);|^[ \t>]*(?:\d+[.)]|[-*+])[ \t]+(?:\[[ xX]\][ \t]+)?/gm;

/** `src` as Markdown blocks. A body the parser cannot read, nested as deep
 * as `NESTING` (`TooDeep`), or whose reading would leave out any of its
 * words (a table row's cells past its header's, or anything not foreseen)
 * is one paragraph of its text: nothing in a comment is hidden from the
 * people reading it, while agents read it whole. */
export function parse(src: string): Block[] {
  const hit = cache.get(src);
  if (hit) return hit;
  const bs = read(src);
  if (cache.size >= CACHED) cache.delete(cache.keys().next().value!);
  cache.set(src, bs);
  return bs;
}

/** The bodies read most recently, so previews and cards share a reading. */
const cache = new Map<string, Block[]>();
const CACHED = 256;

function read(src: string): Block[] {
  const asText = (): Block[] => (src.trim() ? [{ t: "p", c: [{ t: "text", v: src }] }] : []);
  try {
    let ts = (tooWide(src) ? plainMd : md).parse(src, {});
    if (ts.filter(t => t.type === "th_open" || t.type === "td_open").length > MAX_CELLS) ts = plainMd.parse(src, {});
    if (ts.some(t => t.level >= NESTING - 1)) return asText();
    const bs = blocks(nest(ts));
    return said(src.replace(MARKUP, " "), bs.map(blockText).join("\n")) ? bs : asText();
  } catch {
    return asText();
  }
}

/** `c` as text; with `addr`, a link's address follows its text. */
const inlineText = (c: Inline[], addr = false): string =>
  c.map(x => (x.t === "text" || x.t === "code" ? x.v : x.t === "br" ? "\n" : x.t === "link" && addr && x.addr ? `${inlineText(x.c, addr)} (${x.addr})` : inlineText(x.c, addr))).join("");

function blockText(b: Block): string {
  switch (b.t) {
    case "p": case "line": case "h": return inlineText(b.c, true);
    case "pre": return [b.info, b.v].filter(Boolean).join("\n");
    case "quote": return b.c.map(blockText).join("\n");
    case "list": return b.items.map(i => i.c.map(blockText).join("\n")).join("\n");
    case "table": return [b.head, ...b.rows].map(r => r.map(c => inlineText(c, true)).join(" ")).join("\n");
    case "hr": return "";
  }
}

/** `src` with its Markdown syntax taken away: each block's text on lines
 * of its own (a list item, a table row), a link as its text. For previews,
 * which show it as text. */
export function plain(src: string): string {
  return parse(src).map(blockText).filter(s => s !== "").join("\n");
}
