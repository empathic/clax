// Removes the previous build's shell and bridge files (their names change
// with their content, and every file in web/dist is embedded in the binary),
// keeping web/dist/.gitkeep.
import { rmSync } from "node:fs";
const dist = new URL("../dist/", import.meta.url);
rmSync(new URL("_clax/", dist), { recursive: true, force: true });
rmSync(new URL(".vite/", dist), { recursive: true, force: true });
for (const f of ["index.html", "artifact.html"]) rmSync(new URL(f, dist), { force: true });
