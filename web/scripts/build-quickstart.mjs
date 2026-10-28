// Copies the quickstart page (web/quickstart) into web/dist/quickstart, so
// the binary embeds it: only the files in SHIPPED, never the ideas page, its
// data, the README or the .qual records. The copy of index.html leaves out
// its `<script src="ideas-data.js">` tag (IDEAS_TAG), as the file it loads
// does not ship. Copies, never links; a symlinked
// web/dist/quickstart is refused, as clean-dist.mjs refuses a symlinked
// web/dist.
import { cpSync, lstatSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** The quickstart's shipped paths; a trailing "/" is a directory, copied whole. */
export const SHIPPED = ["index.html", "style.css", "icon.svg", "clax.json", "logos/"];

/** index.html's line loading the ideas data, which a shipped copy drops. */
export const IDEAS_TAG = /^[ \t]*<script src="ideas-data\.js"><\/script>\r?\n/m;

/** index.html's tab bar, which a shipped copy drops: its only other tab is
 * the ideas page, which does not ship. */
export const TABS = /^[ \t]*<nav class="tabs"[^>]*>[\s\S]*?<\/nav>\r?\n/m;

const stat = p => { try { return lstatSync(p); } catch (e) { if (e.code === "ENOENT") return null; throw e; } };

function* files(dir, prefix) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.isDirectory()) yield* files(join(dir, e.name), `${prefix}${e.name}/`);
    else yield `${prefix}${e.name}`;
  }
}

/** Copies every SHIPPED path from `src` to `dest`; returns the copied
 * relative paths, sorted. A missing listed path throws. */
export function copyQuickstart(src, dest) {
  if (stat(dest)?.isSymbolicLink()) throw new Error(`build-quickstart: ${dest} is a symlink; refusing to copy outside web/dist. Replace it with a real directory.`);
  const copied = [];
  for (const name of SHIPPED) {
    const dir = name.endsWith("/");
    const rel = dir ? name.slice(0, -1) : name;
    const from = join(src, rel);
    const s = stat(from);
    if (!s || (dir ? !statSync(from).isDirectory() : !s.isFile())) throw new Error(`build-quickstart: ${from} is missing`);
    mkdirSync(dirname(join(dest, rel)), { recursive: true });
    if (name === "index.html") writeFileSync(join(dest, rel), readFileSync(from, "utf8").replace(IDEAS_TAG, "").replace(TABS, ""));
    else cpSync(from, join(dest, rel), { recursive: dir, dereference: false });
    if (dir) copied.push(...files(from, name));
    else copied.push(name);
  }
  return copied.sort();
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const src = fileURLToPath(new URL("../quickstart", import.meta.url));
  const dest = fileURLToPath(new URL("../dist/quickstart", import.meta.url));
  try {
    copyQuickstart(src, dest);
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}
