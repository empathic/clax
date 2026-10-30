// How the `open` tool hands a browser a one-use login link without putting
// it on the opener's command line, where any user of the machine can read
// it (`ps`) and race the browser to redeem it: as `clax open` does
// (crates/clax-core/src/login_page.rs), the link goes into a small page
// under the Clax home's `login/` (mode 0700), in a file of its own (mode
// 0600) named for the code's SHA-256, and the opener is given its path.
// The daemon removes the page when its code is redeemed, and each write
// first removes the pages older than a code lives.

import { createHash } from "node:crypto";
import { chmodSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/** How long a login code may be redeemed, from when it was made. */
export const LOGIN_CODE_TTL_MS = 120_000;

/** The login code in `url`'s query (`code=…`), if any. */
export function codeOf(url: string): string | undefined {
  const query = url.split("?")[1]?.split("#")[0] ?? "";
  const code = query.split("&").find(kv => kv.startsWith("code="))?.slice(5);
  return code ? code : undefined;
}

/** The page's file name: the code's SHA-256, which only its holder can name. */
export function pageName(code: string): string {
  return `${createHash("sha256").update(code).digest("hex")}.html`;
}

/** The page that sends its browser on to `url`, loading nothing else. */
export function page(url: string): string {
  const attr = url.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const script = JSON.stringify(url).replace(/</g, "\\u003c").replace(/>/g, "\\u003e");
  return `<!doctype html>\n<meta charset="utf-8">\n<meta name="referrer" content="no-referrer">\n`
    + `<meta http-equiv="refresh" content="0;url=${attr}">\n<title>Opening Clax</title>\n`
    + `<script>location.replace(${script})</script>\n<p><a href="${attr}">Open Clax</a></p>\n`;
}

/** Writes the page for login link `url` into `dir` (mode 0700) as a new
 * file of mode 0600, after removing the pages older than a code lives, and
 * returns its path. Throws when `url` carries no code or the page cannot be
 * written. */
export function writeLoginPage(dir: string, url: string, now = Date.now()): string {
  const code = codeOf(url);
  if (code === undefined) throw new Error("the link carries no code");
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  chmodSync(dir, 0o700);
  for (const name of readdirSync(dir)) {
    if (!name.endsWith(".html")) continue;
    const p = join(dir, name);
    try {
      if (now - statSync(p).mtimeMs > LOGIN_CODE_TTL_MS) rmSync(p, { force: true });
    } catch {
      // gone already
    }
  }
  const path = join(dir, pageName(code));
  writeFileSync(path, page(url), { flag: "wx", mode: 0o600 });
  return path;
}
