// The classic script (`/_clax/overlay.js`, global `Clax`): the overlay, and
// the loader, so a page or a tag manager installs a Clax container with one
// tag (`Clax.install({...})`, or `window.clax = [["install", {...}]]` before
// the script). The SDK package keeps them apart (`./overlay`, `./loader`).
import { installQueued } from "./loader";

export * from "./main";
export { install, installQueued, type Installed, type Snippet } from "./loader";

// Loaded after a tag manager queued `["install", {...}]`: install it now.
if (typeof window !== "undefined") installQueued(window)?.catch(() => {});
