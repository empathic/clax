// The shell's service worker, served at /sw.js so its scope is the whole
// shell: it makes Clax an installable web app (Add to Home Screen opens it
// standalone). It caches nothing and handles no fetch: every page, and
// every byte of the owner's data, comes from the daemon as it would
// without it.
self.addEventListener("install", () => { self.skipWaiting(); });
self.addEventListener("activate", e => { e.waitUntil(self.clients.claim()); });
