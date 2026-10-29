// Scrubbing (spec 2026-10-29-feedback-loop-design §5.3): the rule set that
// replaces credentials and personal data in what a reporter's browser sends
// (diagnostics, the page URL, the app's context) by `REDACTED`, applied here
// before sending and by Clax again on receipt. The rules are those of
// `crates/clax-core/src/scrub.rs`, whose module documentation states them
// and is their source of truth; `web/sdk/test/scrub-cases.json` is the
// table of cases both test suites run.
//
// Every pattern is ASCII, so reading the text by UTF-16 code unit here
// matches what the Rust reads by byte: a non-ASCII character is never part
// of a pattern, and it is "not a letter or digit" to both.

/** What a scrubbed secret becomes. */
export const REDACTED = "‹redacted›";

/** The key names whose values a secret pair (rule 4) replaces, matched in any case. */
const SECRET_KEYS = ["password", "passwd", "token", "secret", "key", "api_key", "api-key", "access_token", "auth"];

/** The last labels of an `x@y.z` that name a file, not a mail domain (rule 5). */
export const FILE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "js", "mjs", "css", "map", "json", "html", "woff", "woff2"];

const isDigit = (c: number) => c >= 0x30 && c <= 0x39;
const isUpper = (c: number) => c >= 0x41 && c <= 0x5a;
const isLower = (c: number) => c >= 0x61 && c <= 0x7a;
const isAlpha = (c: number) => isUpper(c) || isLower(c);
const isAlnum = (c: number) => isAlpha(c) || isDigit(c);
const isHex = (c: number) => isDigit(c) || (c >= 0x41 && c <= 0x46) || (c >= 0x61 && c <= 0x66);
/** Rust's `u8::is_ascii_whitespace`: space, tab, LF, FF and CR. */
const isWs = (c: number) => c === 0x20 || c === 0x09 || c === 0x0a || c === 0x0c || c === 0x0d;
const oneOf = (chars: string) => (c: number) => chars.includes(String.fromCharCode(c)) && c < 0x80;

const isB64url = (c: number) => isAlnum(c) || c === 0x2d || c === 0x5f;
const tokenPunct = oneOf("-._~+/");
const isB64token = (c: number) => isAlnum(c) || tokenPunct(c);
const emailPunct = oneOf("._%+-");
const isEmailLocal = (c: number) => isAlnum(c) || emailPunct(c);
const runPunct = oneOf("+-_");
const isRun = (c: number) => isAlnum(c) || runPunct(c);
const isSpaceTab = (c: number) => c === 0x20 || c === 0x09;
const isEq = (c: number) => c === 0x3d;
const valueStop = oneOf("&\"',;)");

/** The end of the run of code units from `i` that `f` accepts. */
function runEnd(s: string, i: number, f: (c: number) => boolean): number {
  let j = i;
  while (j < s.length && f(s.charCodeAt(j))) j++;
  return j;
}

/** Whether a word starts at `i`: the previous character is not an ASCII letter or digit. */
const wordStart = (s: string, i: number) => i === 0 || !isAlnum(s.charCodeAt(i - 1));

/** Whether `s` holds `word` (lower case ASCII) at `i`, in any case. */
function startsWithCi(s: string, i: number, word: string): boolean {
  if (s.length < i + word.length) return false;
  for (let k = 0; k < word.length; k++) {
    let c = s.charCodeAt(i + k);
    if (isUpper(c)) c += 0x20;
    if (c !== word.charCodeAt(k)) return false;
  }
  return true;
}

type Span = [number, number];

/** Rule 1: the token after `bearer` or `basic`. */
function bearer(s: string, i: number): Span | null {
  if (!wordStart(s, i)) return null;
  const scheme = ["bearer", "basic"].find(w => startsWithCi(s, i, w));
  if (!scheme) return null;
  const after = i + scheme.length;
  const start = runEnd(s, after, isSpaceTab);
  if (start === after) return null;
  const body = runEnd(s, start, isB64token);
  if (body - start < 8) return null;
  let digit = false, lower = false, upperAfterFirst = false;
  for (let k = start; k < body; k++) {
    const c = s.charCodeAt(k);
    digit ||= isDigit(c);
    lower ||= isLower(c);
    if (k > start) upperAfterFirst ||= isUpper(c);
  }
  if (!(digit || (lower && upperAfterFirst))) return null;
  return [start, runEnd(s, body, isEq)];
}

/** Rule 2: a `cx?_` token. */
function claxToken(s: string, i: number): Span | null {
  if (!wordStart(s, i) || (i > 0 && s.charCodeAt(i - 1) === 0x5f) || s.length < i + 4) return null;
  if (!(s.charCodeAt(i) === 0x63 && s.charCodeAt(i + 1) === 0x78 && isLower(s.charCodeAt(i + 2)) && s.charCodeAt(i + 3) === 0x5f)) return null;
  const end = runEnd(s, i + 4, isB64url);
  return end - (i + 4) >= 16 ? [i, end] : null;
}

/** Rule 3: a JWT-shaped string. */
function jwt(s: string, i: number): Span | null {
  if ((i > 0 && isB64url(s.charCodeAt(i - 1))) || !s.startsWith("eyJ", i)) return null;
  let at = i;
  for (let part = 0; part < 3; part++) {
    if (part > 0) {
      if (s.charCodeAt(at) !== 0x2e) return null;
      at++;
    }
    const end = runEnd(s, at, isB64url);
    if (end - at < 8) return null;
    at = end;
  }
  return [i, at];
}

/** Rule 4: the value of a secret pair. */
function secretPair(s: string, i: number): Span | null {
  if (!wordStart(s, i)) return null;
  const key = SECRET_KEYS.find(k => startsWithCi(s, i, k));
  if (!key) return null;
  let at = i + key.length;
  const quotedKey = s[at] === '"' || s[at] === "'";
  if (quotedKey) at++;
  const sep = s[at];
  if (!(sep === "=" || (sep === ":" && (quotedKey || key !== "key")))) return null;
  at = runEnd(s, at + 1, isSpaceTab);
  const quote = s[at];
  if (quote === '"' || quote === "'") {
    const start = at + 1;
    let end = start;
    while (end < s.length && s[end] !== quote) end += s[end] === "\\" ? 2 : 1;
    end = Math.min(end, s.length);
    return end > start ? [start, end] : null;
  }
  const cred = bearer(s, at);
  if (cred) return [at, cred[1]];
  const end = runEnd(s, at, c => !(isWs(c) || valueStop(c)));
  return end > at ? [at, end] : null;
}

/** Rule 5: an email address. */
function email(s: string, i: number): Span | null {
  if (i > 0 && isEmailLocal(s.charCodeAt(i - 1))) return null;
  const local = runEnd(s, i, isEmailLocal);
  if (local === i || s[local] !== "@") return null;
  const start = local + 1;
  let end = runEnd(s, start, c => isAlnum(c) || c === 0x2e || c === 0x2d);
  while (end > start && (s[end - 1] === "." || s[end - 1] === "-")) end--;
  const labels = s.slice(start, end).split(".");
  if (labels.length < 2 || labels.some(l => l === "")) return null;
  const tld = labels[labels.length - 1];
  const file = FILE_EXTENSIONS.includes(tld.toLowerCase());
  const alpha = [...tld].every(ch => isAlpha(ch.charCodeAt(0)));
  return tld.length >= 2 && alpha && !file ? [i, end] : null;
}

/** Whether `part` of a `/`-joined run is a path word: 3 or more of `a-z - _` only. */
const isPathWord = (part: string) => part.length >= 3 && /^[a-z_-]+$/.test(part);

/** Rule 6: a long hex or base64 run. */
function longRun(s: string, i: number): Span | null {
  if (i > 0 && isRun(s.charCodeAt(i - 1))) return null;
  const end = runEnd(s, i, isRun);
  if (end === i) return null;
  const prev = i > 0 ? s[i - 1] : "";
  if (prev !== "/" && prev !== "." && prev !== ":") {
    const ext = runEnd(s, i, c => isRun(c) || c === 0x2f);
    const run = s.slice(i, ext);
    if (ext > end && run.length >= 40 && /[A-Z]/.test(run) && /[a-z]/.test(run) && /[0-9]/.test(run) && !run.split("/").some(isPathWord)) {
      return [i, runEnd(s, ext, isEq)];
    }
  }
  let hex = end - i >= 32, digit = false;
  for (let k = i; k < end; k++) {
    const c = s.charCodeAt(k);
    hex &&= isHex(c);
    digit ||= isDigit(c);
  }
  const b64 = end - i >= 40 && digit;
  return hex || b64 ? [i, runEnd(s, end, isEq)] : null;
}

const urlStop = oneOf("\"'<>`");
const urlTrail = oneOf(".,;:!?)]}");

/** The end of the URL that starts at `i`, if one does. */
function urlAt(s: string, i: number): number | null {
  if (!wordStart(s, i)) return null;
  const scheme = ["https://", "http://"].find(w => startsWithCi(s, i, w));
  if (!scheme) return null;
  const from = i + scheme.length;
  let end = runEnd(s, from, c => !(isWs(c) || urlStop(c)));
  while (end > from && urlTrail(s.charCodeAt(end - 1))) end--;
  // A stack frame's `:line:column` follows the script's URL.
  for (let n = 0; n < 2; n++) {
    let d = end;
    while (d > from && isDigit(s.charCodeAt(d - 1))) d--;
    if (d === end || d <= from || s[d - 1] !== ":") break;
    end = d - 1;
  }
  return end;
}

/** The span at `i` that the first matching rule replaces. */
function findAt(s: string, i: number): Span | null {
  return bearer(s, i) ?? claxToken(s, i) ?? jwt(s, i) ?? secretPair(s, i) ?? email(s, i) ?? longRun(s, i);
}

/** `s` with every match of the rules replaced by `REDACTED`, read left to
 * right: at each position an `http://` or `https://` URL is replaced by
 * its `scrubUrl` form, else the first rule that matches replaces what it
 * names, and the scan resumes after it. Text already holding `REDACTED` in
 * place of a secret scrubs to itself. */
export function scrub(s: string): string {
  let out = "";
  let copied = 0;
  let i = 0;
  while (i < s.length) {
    const urlEnd = urlAt(s, i);
    if (urlEnd !== null) {
      const url = s.slice(i, urlEnd);
      const scrubbed = scrubUrl(url);
      if (scrubbed !== url) {
        out += s.slice(copied, i) + scrubbed;
        copied = urlEnd;
      }
      i = urlEnd;
      continue;
    }
    const m = findAt(s, i);
    if (!m) {
      i++;
      continue;
    }
    const [start, end] = m;
    if (s.slice(start, end) !== REDACTED) {
      out += s.slice(copied, start) + REDACTED;
      copied = end;
    }
    i = Math.max(end, i + 1);
  }
  return copied === 0 ? s : out + s.slice(copied);
}

/** `s` with each lone surrogate replaced by U+FFFD, so that its JSON is
 * text Clax reads: `JSON.stringify` writes a lone surrogate as an escape
 * (`\ud83d`) that a strict JSON parser refuses. Use it on every string a
 * report's context or diagnostics carries. */
export function wellFormed(s: string): string {
  const native = (s as { toWellFormed?: () => string }).toWellFormed;
  if (typeof native === "function") return native.call(s);
  return s.replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, "�");
}

/** `s` cut to at most `max` UTF-16 code units, never inside a surrogate
 * pair; a cut string ends in `…`. */
export function cut(s: string, max: number): string {
  if (s.length <= max) return s;
  let end = Math.max(0, max - 1);
  const last = s.charCodeAt(end - 1);
  if (end > 0 && last >= 0xd800 && last <= 0xdbff) end--;
  return `${s.slice(0, end)}…`;
}

/** `s` split at the first `sep`, or `null` without one. */
function splitOnce(s: string, sep: string): [string, string] | null {
  const at = s.indexOf(sep);
  return at < 0 ? null : [s.slice(0, at), s.slice(at + sep.length)];
}

/** The query `q` with its keys kept and every non-empty value replaced. */
function scrubQuery(q: string): string {
  return q.split("&").map(entry => {
    const kv = splitOnce(entry, "=");
    if (!kv) return scrub(entry);
    return `${scrub(kv[0])}=${kv[1] ? REDACTED : ""}`;
  }).join("&");
}

/** `url` with its scheme, host, path and query keys kept and every query
 * value replaced by `REDACTED` (an empty value stays empty). The path and a
 * query entry with no `=` are scrubbed; a user name or password before the
 * host is replaced. The fragment is read as a route: the part after its
 * first `?` is a query (`#/reset?code=…`); a fragment with no `?` but an
 * `=` is a query as a whole (`#state=…&code=…`); anything else in it is
 * scrubbed. A string with no `scheme://` is treated as a path with an
 * optional query and fragment. */
export function scrubUrl(url: string): string {
  const fragSplit = splitOnce(url, "#");
  const [beforeFrag, frag] = fragSplit ?? [url, null];
  const querySplit = splitOnce(beforeFrag, "?");
  const [base, query] = querySplit ?? [beforeFrag, null];
  let out = "";
  const at = base.indexOf("://");
  if (at >= 0) {
    out += base.slice(0, at + 3);
    const rest = base.slice(at + 3);
    const slash = rest.indexOf("/");
    const [authority, path] = slash < 0 ? [rest, ""] : [rest.slice(0, slash), rest.slice(slash)];
    const userAt = authority.lastIndexOf("@");
    out += userAt < 0 ? authority : `${REDACTED}@${authority.slice(userAt + 1)}`;
    out += scrub(path);
  } else {
    out += scrub(base);
  }
  if (query !== null) out += `?${scrubQuery(query)}`;
  if (frag !== null) {
    out += "#";
    const routeSplit = splitOnce(frag, "?");
    if (routeSplit) out += `${scrub(routeSplit[0])}?${scrubQuery(routeSplit[1])}`;
    else if (frag.includes("=")) out += scrubQuery(frag);
    else out += scrub(frag);
  }
  return out;
}
