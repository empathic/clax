/** The manifest a build of the Chrome extension ships (extension-manifest.mjs). `web` is the web/ directory, with a trailing slash. */
export function extensionManifest(web: string, options: { test: boolean }): Record<string, unknown> & { version: string; key?: string; host_permissions?: string[] };
