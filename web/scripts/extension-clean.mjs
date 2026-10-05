// Empties an extension build's output directory before a build, keeping its
// committed .gitkeep. Never touches anything outside it: a symlinked output
// directory is refused, and a symlinked entry is unlinked, not followed.
import { lstatSync, mkdirSync, readdirSync, rmSync, unlinkSync } from "node:fs";

export function cleanOut(out) {
  let s = null;
  try { s = lstatSync(out); } catch (e) { if (e.code !== "ENOENT") throw e; }
  if (s?.isSymbolicLink()) throw new Error(`${out} is a symlink; refusing to build outside web/. Replace it with a real directory.`);
  mkdirSync(out, { recursive: true });
  for (const e of readdirSync(out, { withFileTypes: true })) {
    if (e.name === ".gitkeep") continue;
    if (e.isSymbolicLink()) unlinkSync(`${out}/${e.name}`);
    else rmSync(`${out}/${e.name}`, { recursive: true, force: true });
  }
}
