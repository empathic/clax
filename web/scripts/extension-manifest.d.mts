type Manifest = Record<string, unknown> & { version: string; version_name?: string; key?: string; host_permissions?: string[] };
/** The manifest a build of the Chrome extension ships (extension-manifest.mjs). `web` is the web/ directory, with a trailing slash. */
export function extensionManifest(web: string, options: { test: boolean }): Manifest;
/** `manifest` with `version_name` set to the stamp of the build in `dir` (extension-manifest.mjs). */
export function stamped(dir: string, manifest: Manifest): Manifest;
