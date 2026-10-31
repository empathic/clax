// The client package's (@empathic/clax-client) name and entries: web/shell's
// code that Clax's own clients (web/extension) share with the shell, behind
// one public surface. It is AGPL-3.0-only, as web/shell is; a page outside
// Clax uses @empathic/clax-sdk (sdk-entries.mjs), which is LGPL-2.1-only and
// bundles none of it. Not published: each entry resolves to its source in
// the extension's build (build-extension.mjs) and the unit tests
// (vitest.config.ts), and tsconfig.json's `paths` say the same to
// TypeScript. web/.oxlintrc.json holds web/extension to the two packages.
import { join } from "node:path";

/** The package's name. */
export const CLIENT_NAME = "@empathic/clax-client";

/** The package's entries: its export and the source. */
export const CLIENT_ENTRIES = [
  { path: "./sync", src: "shell/client/sync.ts" },
  { path: "./stream", src: "shell/client/stream.ts" },
  { path: "./models", src: "shell/client/models.ts" },
  { path: "./ui", src: "shell/client/ui.ts" },
  { path: "./inbox", src: "shell/client/inbox.ts" },
  { path: "./composer", src: "shell/client/composer.ts" },
  { path: "./testing", src: "shell/src/test/client.ts" },
  // The theme's stylesheet, which a client's page imports once.
  { path: "./theme.css", src: "shell/src/theme.css" },
  // Components a client loads lazily (`import()`), each its own chunk.
  { path: "./roster", src: "shell/client/roster.ts" },
  { path: "./send-menu", src: "shell/client/send-menu.ts" },
];

/** Vite's `resolve.alias` from each entry's specifier, exactly, to its
 * source under `web` (an absolute path). */
export function clientSourceAlias(web) {
  return CLIENT_ENTRIES.map(e => {
    const spec = `${CLIENT_NAME}/${e.path.slice(2)}`;
    return { find: new RegExp(`^${spec.replace(/[/.]/g, "\\$&")}$`), replacement: join(web, e.src) };
  });
}
