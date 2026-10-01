// Removes the previous build's shell and bridge files (their names change
// with their content, and every file in web/dist is embedded in the binary),
// keeping web/dist/.gitkeep. Never touches anything outside web/dist: a
// symlink among the removed paths is unlinked, not followed, and a symlinked
// web/dist is refused.
import { lstatSync, rmSync, unlinkSync } from "node:fs";
import { fileURLToPath } from "node:url";

const dist = fileURLToPath(new URL("../dist", import.meta.url));
const stat = p => { try { return lstatSync(p); } catch (e) { if (e.code === "ENOENT") return null; throw e; } };

if (stat(dist)?.isSymbolicLink()) {
  console.error(`clean-dist: ${dist} is a symlink; refusing to clean outside web/dist. Replace it with a real directory.`);
  process.exit(1);
}
for (const name of ["_clax", ".vite", "index.html", "artifact.html"]) {
  const p = `${dist}/${name}`;
  const s = stat(p);
  if (!s) continue;
  if (s.isSymbolicLink()) unlinkSync(p);
  else rmSync(p, { recursive: true, force: true });
}
