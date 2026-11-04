// The classic script (`/_clax/overlay.js`, global `Clax`): the overlay, and
// the loader, so a page or a tag manager installs a Clax container with one
// tag: `<script async src="{instance}/_clax/overlay.js" data-container="…"
// data-auth="…">`, `Clax.install({...})`, or `window.clax = [["install",
// {...}]]` before the script. The SDK package keeps them apart (`./overlay`,
// `./loader`).
import { installFromTag, installQueued } from "./loader";

export * from "./main";
export { CARRIER_ATTR, SITE_INSTALLED, claim, install, installQueued, snippetFromTag, type Carrier, type Installed, type Snippet } from "./loader";

// Loaded by a tag with data-container, or after a tag manager queued
// `["install", {...}]`: install it now. `document.currentScript` is only
// set while this runs, so the tag is read first.
if (typeof window !== "undefined") (installFromTag(document) ?? installQueued(window))?.catch(() => {});
