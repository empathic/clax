// The sanitized snapshot of a live page (spec 2026-10-05 §8.2): the DOM as
// HTML with no script, no event handler, no form value, no hidden input,
// no comment and no dangerous URL; styles taken from the CSSOM (so rules
// CSS-in-JS inserted are kept) with their URLs made absolute; open shadow
// roots as declarative shadow DOM; embedded content as sized placeholders.
// Every check is made on the lowercase name the HTML parser will see when the
// snapshot is loaded again, and names that would not parse back as written are
// dropped. The daemon also serves it with a policy that runs no page script (§8.4).
import { OVERLAY_TAG } from "../../../bridge/src/anchor";

export const MAX_ELEMENTS = 100_000;
export const MAX_BYTES = 8 * 1024 * 1024;
export const DEADLINE_MS = 1500;
/** Nesting past this depth is reported as too large instead of overflowing the stack. */
const MAX_DEPTH = 1000;

export type Snapshot = { html: string; error: null } | { html: string; error: "too_large" };
type Opts = { skip?: Element[]; maxElements?: number; maxBytes?: number; deadlineMs?: number; now?: () => number };

const HTML_NS = "http://www.w3.org/1999/xhtml";
const DROP = new Set(["script", "noscript", "base", "template", "portal", "noembed", "noframes"]);
const PLACEHOLDER = new Set(["iframe", "frame", "frameset", "fencedframe", "object", "embed", "applet", "canvas", "video", "audio"]);
const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);
/** Elements whose content the parser reads as raw text; written as `pre` so escaped text reads back as text. */
const RAW = new Set(["xmp", "plaintext"]);
const URL_ATTRS = new Set(["href", "src", "poster", "background", "xlink:href", "cite", "longdesc"]);
const DROP_ATTRS = new Set(["srcdoc", "nonce", "integrity", "action", "formaction", "ping", "xml:base", "selected", "checked"]);
const FORM_VALUE = new Set(["input", "button", "option", "textarea"]);
const TAG_NAME = /^[a-z][a-z0-9-]*$/i;
const ATTR_NAME = /^[a-z_:][-\w:.]*$/i;
const TOO_LARGE = Symbol("too large");

const escText = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const escAttr = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;");

/** The UTF-8 length of `s`, a lone surrogate counting as the U+FFFD that replaces it. */
function utf8Length(s: string): number {
  let n = s.length;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) continue;
    if (c < 0x800) n += 1;
    else if (c >= 0xd800 && c < 0xdc00 && (s.charCodeAt(i + 1) & 0xfc00) === 0xdc00) { n += 2; i++; }
    else n += 2;
  }
  return n;
}

/** `u` made absolute against `base`, or null unless it is http(s) or a data image; a bare fragment is kept as is. */
function absolute(u: string, base: string): string | null {
  const t = u.trim();
  if (t.startsWith("#")) return t;
  try {
    const abs = new URL(t, base).href;
    return /^(https?:|data:image\/)/i.test(abs) ? abs : null;
  } catch {
    return null;
  }
}

/** Each candidate of a srcset kept with its absolute URL; a URL runs to whitespace (trailing commas end it), its descriptors to a comma. */
function srcset(value: string, base: string): string | null {
  const out: string[] = [];
  let rest = value;
  for (;;) {
    rest = rest.replace(/^[\s,]+/, "");
    if (!rest) break;
    let url = /^\S+/.exec(rest)![0];
    rest = rest.slice(url.length);
    let desc = "";
    if (url.endsWith(",")) url = url.replace(/,+$/, "");
    else {
      desc = /^[^,]*/.exec(rest)![0];
      rest = rest.slice(desc.length);
    }
    const abs = absolute(url, base);
    if (abs) out.push(`${abs} ${desc.trim()}`.trim());
  }
  return out.join(", ") || null;
}

function cssUrls(text: string, base: string): string {
  return text.replace(/url\(\s*(['"]?)([^'")]*)\1\s*\)/gi, (_m, _q, u: string) => `url("${(absolute(u, base) ?? "").replace(/"/g, "%22")}")`);
}

function rulesOf(sheet: CSSStyleSheet | null | undefined): string | null {
  if (!sheet) return null;
  try {
    return [...sheet.cssRules].map(r => r.cssText).join("\n");
  } catch {
    return null;
  }
}

class Writer {
  private parts: string[] = [];
  private bytes = 0;
  private count = 0;
  private depth = 0;
  private readonly started: number;
  constructor(private readonly o: Required<Omit<Opts, "skip">> & { skip: Set<Element> }, private readonly base: string) {
    this.started = o.now();
  }
  emit(s: string) {
    this.bytes += utf8Length(s);
    if (this.bytes > this.o.maxBytes) throw TOO_LARGE;
    this.parts.push(s);
  }
  tick() {
    if (++this.count > this.o.maxElements || this.o.now() - this.started > this.o.deadlineMs) throw TOO_LARGE;
  }
  text(): string {
    return this.parts.join("");
  }

  /** A `<style>` holding `css`; a `<` that could open a tag (and so end the element or, inside SVG, add one) is written as a CSS escape. */
  style(css: string, base: string, media?: string | null) {
    const m = media ? ` media="${escAttr(media)}"` : "";
    this.emit(`<style${m}>${cssUrls(css, base).replace(/<(?=[a-z/!?])/gi, "\\3c ")}</style>`);
  }

  element(el: Element): void {
    this.tick();
    const name = el.localName;
    const tag = name.toLowerCase();
    if (this.o.skip.has(el) || tag === OVERLAY_TAG || DROP.has(tag) || !TAG_NAME.test(name)) return;
    if (tag === "input" && (el as HTMLInputElement).type === "hidden") return;
    if (tag === "meta" && (el.hasAttribute("http-equiv") || el.hasAttribute("charset"))) return;
    if ((tag === "set" || tag === "animate") && /(^|:)href$/i.test(el.getAttribute("attributeName") ?? "")) return;
    const media = el.getAttribute("media");
    if (tag === "link") {
      if (!/(^|\s)stylesheet(\s|$)/i.test(el.getAttribute("rel") ?? "")) return;
      const sheet = (el as HTMLLinkElement).sheet as CSSStyleSheet | null;
      if (sheet?.disabled) return;
      const href = absolute(el.getAttribute("href") ?? "", this.base);
      const rules = rulesOf(sheet);
      if (rules !== null) this.style(rules, sheet?.href ?? href ?? this.base, media);
      else if (href) this.emit(`<link rel="stylesheet" href="${escAttr(href)}"${media ? ` media="${escAttr(media)}"` : ""}>`);
      return;
    }
    if (tag === "style") {
      const sheet = (el as HTMLStyleElement).sheet as CSSStyleSheet | null;
      if (!sheet?.disabled) this.style(rulesOf(sheet) ?? el.textContent ?? "", this.base, media);
      return;
    }
    if (PLACEHOLDER.has(tag)) {
      const r = el.getBoundingClientRect();
      this.emit(`<div data-clax-placeholder="${tag}" style="display:inline-block;width:${Math.round(r.width)}px;height:${Math.round(r.height)}px;background:rgba(128,128,128,.15)"></div>`);
      return;
    }
    if (++this.depth > MAX_DEPTH) throw TOO_LARGE;
    const out = RAW.has(tag) ? "pre" : name;
    this.emit(`<${out}`);
    for (const a of [...el.attributes]) {
      const n = a.name.toLowerCase();
      if (!ATTR_NAME.test(a.name) || n.startsWith("on") || a.localName.toLowerCase().startsWith("on") || DROP_ATTRS.has(n)) continue;
      if (n === "value" && FORM_VALUE.has(tag)) continue;
      let value: string | null = a.value;
      if (URL_ATTRS.has(n)) value = absolute(value, this.base);
      else if (n === "srcset") value = srcset(value, this.base);
      else if (n === "style") value = cssUrls(value, this.base);
      if (value !== null) this.emit(` ${a.name}="${escAttr(value)}"`);
    }
    // The chosen option and checked boxes are the page's state, not values typed into it.
    if (tag === "option" && (el as HTMLOptionElement).selected) this.emit(` selected=""`);
    if (tag === "input" && (el as HTMLInputElement).checked) this.emit(` checked=""`);
    this.emit(">");
    if (tag === "head") {
      this.emit(`<meta charset="utf-8">`);
      for (const s of el.ownerDocument.adoptedStyleSheets ?? []) this.style(rulesOf(s) ?? "", this.base);
    }
    if (!(el.namespaceURI === HTML_NS && VOID.has(tag))) {
      if (tag !== "textarea") {
        const shadow = (el as HTMLElement).shadowRoot;
        if (shadow) {
          this.emit(`<template shadowrootmode="open">`);
          for (const s of shadow.adoptedStyleSheets ?? []) this.style(rulesOf(s) ?? "", this.base);
          this.children(shadow);
          this.emit(`</template>`);
        }
        this.children(el);
      }
      this.emit(`</${out}>`);
    }
    this.depth--;
  }

  children(n: ParentNode): void {
    for (const c of n.childNodes) {
      if (c.nodeType === Node.TEXT_NODE) this.emit(escText((c as Text).data));
      else if (c.nodeType === Node.ELEMENT_NODE) this.element(c as Element);
    }
  }
}

/** A minimal page standing in for a snapshot past a cap. */
function placeholderPage(title: string): string {
  const t = escText(title.slice(0, 200));
  return `<!doctype html><html><head><meta charset="utf-8"><title>${t}</title></head><body><p>Snapshot unavailable: the page is too large. (${t})</p></body></html>`;
}

/**
 * Serializes `doc` under the caps (elements, UTF-8 bytes of HTML, time on `now`).
 * Past any cap, or nested deeper than the serializer follows, it returns the
 * placeholder page with `error: "too_large"`.
 */
export function serializeSnapshot(doc: Document, opts: Opts = {}): Snapshot {
  const o = {
    skip: new Set(opts.skip ?? []),
    maxElements: opts.maxElements ?? MAX_ELEMENTS,
    maxBytes: opts.maxBytes ?? MAX_BYTES,
    deadlineMs: opts.deadlineMs ?? DEADLINE_MS,
    now: opts.now ?? (() => performance.now()),
  };
  const w = new Writer(o, doc.baseURI);
  try {
    w.emit("<!doctype html>");
    w.element(doc.documentElement);
    return { html: w.text(), error: null };
  } catch (e) {
    if (e === TOO_LARGE) return { html: placeholderPage(doc.title), error: "too_large" };
    throw e;
  }
}
