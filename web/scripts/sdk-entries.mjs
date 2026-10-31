// The SDK package's (@empathic/clax-sdk) name and entries, which
// build-sdk.mjs builds, and the alias Clax's own consumers of the SDK
// (web/extension: build-extension.mjs, vitest.config.ts; tsconfig.json's
// `paths` say the same to TypeScript) resolve its specifiers with: each
// entry to its source, so they import the SDK by its name, as an app would.
// Kept apart from build-sdk.mjs, which loads TypeScript.
import { join } from "node:path";

/** The package's name on npm. */
export const PACKAGE_NAME = "@empathic/clax-sdk";

/** The package's entries: its export, the file it builds to and the source. */
export const ENTRIES = [
  { path: ".", name: "index", src: "sdk/src/index.ts" },
  { path: "./overlay", name: "overlay", src: "overlay/src/main.ts" },
  { path: "./service", name: "service", src: "sdk/src/service.ts" },
  { path: "./anchor", name: "anchor", src: "sdk/src/anchor.ts" },
];

/** The specifier an app imports `entry` by. */
export const specifier = entry => (entry.path === "." ? PACKAGE_NAME : `${PACKAGE_NAME}/${entry.path.slice(2)}`);

/** Vite's `resolve.alias` from each entry's specifier, exactly, to its
 * source under `web` (an absolute path). */
export function sdkSourceAlias(web) {
  return ENTRIES.map(e => ({ find: new RegExp(`^${specifier(e).replace(/[/.]/g, "\\$&")}$`), replacement: join(web, e.src) }));
}
