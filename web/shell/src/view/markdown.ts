// Comment bodies as Markdown: CommonMark with GitHub's extensions (tables,
// strikethrough, task lists, bare URLs made links), with no images and no
// raw HTML.
//
// The text is untrusted. `marked`'s lexer only tokenizes it; nothing here
// makes HTML. `parse` turns the tokens into the closed tree below (`Block`,
// `Inline`), which `ui/Markdown.svelte` renders as elements whose text is
// always text. Raw HTML in a comment becomes text, an image becomes a link
// to its address (no remote fetch), and a link is kept only when its
// address is absolute `http:`, `https:` or `mailto:` (`safeHref`); any
// other link is its text alone. This module is loaded on demand
// (`md-load.svelte.ts`): the shell's entries do not carry the lexer.
import { Lexer, type Token, type Tokens } from "marked";

/** Text-level content. A `link`'s `href` has passed `safeHref`. */
export type Inline =
  | { t: "text"; v: string }
  | { t: "code"; v: string }
  | { t: "em" | "strong" | "del"; c: Inline[] }
  | { t: "link"; href: string; c: Inline[] }
  | { t: "br" };

export type Align = "left" | "center" | "right" | null;

/** One item of a list: a task item has a read-only box. */
export type Item = { task: boolean; checked: boolean; c: Block[] };

export type Level = 1 | 2 | 3 | 4 | 5 | 6;

/** Block-level content. `line` is a tight list item's text, set in the
 * item without a paragraph of its own. */
export type Block =
  | { t: "p" | "line"; c: Inline[] }
  | { t: "h"; level: Level; c: Inline[] }
  | { t: "pre"; v: string }
  | { t: "quote"; c: Block[] }
  | { t: "list"; ordered: boolean; start: number | null; items: Item[] }
  | { t: "table"; align: Align[]; head: Inline[][]; rows: Inline[][][] }
  | { t: "hr" };

const SCHEMES = new Set(["http:", "https:", "mailto:"]);

/** `raw` as a link address when it is an absolute `http:`, `https:` or
 * `mailto:` URL (as the URL parser reads it, so case, entities left
 * undecoded, whitespace and control characters cannot smuggle another
 * scheme through), else null. The address returned is the parser's own
 * serialization. */
export function safeHref(raw: string): string | null {
  let u: URL;
  try { u = new URL(raw); } catch { return null; }
  return SCHEMES.has(u.protocol) ? u.href : null;
}

// The named character references a comment is likely to hold; `marked`
// decodes the numeric ones in text itself. Any other stays as typed.
const NAMED: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: "\"", apos: "'", nbsp: " " };
const decode = (s: string): string => s.replace(/&(amp|lt|gt|quot|apos|nbsp);/g, (_, n: string) => NAMED[n]);

/** Every `marked` option the lexer reads, so its global defaults (which
 * another module may change with `marked.use`) never apply. */
const OPTIONS = { gfm: true, breaks: false, pedantic: false, silent: true, async: false } as const;

/** Nesting deeper than this is shown as its source text: a tree the
 * renderer walks recursively stays shallow. */
const DEPTH = 24;

function inline(tokens: Token[] | undefined, inLink: boolean, depth: number): Inline[] {
  const out: Inline[] = [];
  const text = (v: string) => {
    if (!v) return;
    const last = out.at(-1);
    if (last?.t === "text") last.v += v;
    else out.push({ t: "text", v });
  };
  for (const tok of tokens ?? []) {
    if (depth > DEPTH) { text(tok.raw); continue; }
    switch (tok.type) {
      case "text":
        if ("tokens" in tok && tok.tokens?.length) out.push(...inline(tok.tokens, inLink, depth + 1));
        else text(tok.escaped ? tok.text : decode(tok.text));
        break;
      case "escape": text(tok.text); break;
      case "codespan": out.push({ t: "code", v: tok.text }); break;
      case "br": out.push({ t: "br" }); break;
      case "em": case "strong": case "del":
        out.push({ t: tok.type, c: inline(tok.tokens, inLink, depth + 1) });
        break;
      case "link": {
        const c = inline(tok.tokens, true, depth + 1);
        const href = inLink ? null : safeHref(decode(tok.href));
        if (href) out.push({ t: "link", href, c });
        else for (const x of c) { if (x.t === "text") text(x.v); else out.push(x); }
        break;
      }
      case "image": {
        // Never an image: a link to it, named by its alt text or its address.
        const label = (tok as Tokens.Image).text || tok.href;
        const href = inLink ? null : safeHref(decode(tok.href));
        if (href) out.push({ t: "link", href, c: [{ t: "text", v: label }] });
        else text(label);
        break;
      }
      // Raw HTML, and anything else: its source, as text.
      default: text(tok.raw);
    }
  }
  return out;
}

const ALIGN = new Set(["left", "center", "right"]);

function blocks(tokens: Token[], depth: number, tight = false): Block[] {
  const out: Block[] = [];
  for (const tok of tokens) {
    if (depth > DEPTH) { if (tok.raw.trim()) out.push({ t: "p", c: [{ t: "text", v: tok.raw.trim() }] }); continue; }
    switch (tok.type) {
      case "space": case "def": case "checkbox": break;
      case "paragraph": out.push({ t: tight ? "line" : "p", c: inline(tok.tokens, false, depth + 1) }); break;
      case "text":
        out.push({ t: tight ? "line" : "p", c: "tokens" in tok && tok.tokens ? inline(tok.tokens, false, depth + 1) : [{ t: "text", v: decode(tok.text) }] });
        break;
      case "heading":
        out.push({ t: "h", level: Math.min(6, Math.max(1, tok.depth)) as Level, c: inline(tok.tokens, false, depth + 1) });
        break;
      case "code": out.push({ t: "pre", v: tok.text }); break;
      case "blockquote": out.push({ t: "quote", c: blocks(tok.tokens ?? [], depth + 1) }); break;
      case "hr": out.push({ t: "hr" }); break;
      case "list": {
        const l = tok as Tokens.List;
        const start = l.ordered && typeof l.start === "number" && Number.isSafeInteger(l.start) && l.start !== 1 ? l.start : null;
        out.push({
          t: "list", ordered: l.ordered, start,
          items: l.items.map(i => ({ task: !!i.task, checked: !!i.task && !!i.checked, c: blocks(i.tokens, depth + 1, !l.loose) })),
        });
        break;
      }
      case "table": {
        const t = tok as Tokens.Table;
        out.push({
          t: "table",
          align: t.align.map(a => (a && ALIGN.has(a) ? a : null)),
          head: t.header.map(c => inline(c.tokens, false, depth + 1)),
          rows: t.rows.map(r => r.map(c => inline(c.tokens, false, depth + 1))),
        });
        break;
      }
      // Raw HTML, and anything else: its source, as text.
      default: {
        const v = tok.raw.replace(/\n+$/, "");
        if (v.trim()) out.push({ t: "p", c: [{ t: "text", v }] });
      }
    }
  }
  return out;
}

/** `src` as Markdown blocks; a body the lexer cannot read is one
 * paragraph of its text. */
export function parse(src: string): Block[] {
  try {
    return blocks(new Lexer(OPTIONS).lex(src), 0);
  } catch {
    return src.trim() ? [{ t: "p", c: [{ t: "text", v: src }] }] : [];
  }
}

const inlineText = (c: Inline[]): string =>
  c.map(x => (x.t === "text" || x.t === "code" ? x.v : x.t === "br" ? "\n" : x.t === "link" ? inlineText(x.c) : inlineText(x.c))).join("");

function blockText(b: Block): string {
  switch (b.t) {
    case "p": case "line": case "h": return inlineText(b.c);
    case "pre": return b.v;
    case "quote": return b.c.map(blockText).join("\n");
    case "list": return b.items.map(i => i.c.map(blockText).join("\n")).join("\n");
    case "table": return [b.head, ...b.rows].map(r => r.map(inlineText).join(" ")).join("\n");
    case "hr": return "";
  }
}

/** `src` with its Markdown syntax taken away: each block's text on lines
 * of its own (a list item, a table row), a link as its text. For previews,
 * which show it as text. */
export function plain(src: string): string {
  return parse(src).map(blockText).filter(s => s !== "").join("\n");
}
