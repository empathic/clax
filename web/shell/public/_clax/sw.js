// The shell's service worker, served at /sw.js so its scope is the whole
// shell: it makes Clax an installable web app (Add to Home Screen opens it
// standalone) and shows the daemon's Web Push messages. It caches nothing
// and handles no fetch: every page, and every byte of the owner's data,
// comes from the daemon as it would without it.
//
// A push's payload is `{title, url, tag}` (crates/clax-http/src/web_push.rs):
// a one-line title, the shell path a tap opens, and a tag that a newer push
// about the same thing replaces. A tap focuses an open Clax window and
// takes it there, or opens one.
self.addEventListener("install", () => { self.skipWaiting(); });
self.addEventListener("activate", e => { e.waitUntil(self.clients.claim()); });

/** A path of this origin's, else the inbox. */
const shellPath = u => (typeof u === "string" && u.startsWith("/") && !u.startsWith("//") ? u : "/inbox");

self.addEventListener("push", e => {
  let m = {};
  try { m = e.data ? e.data.json() : {}; } catch { m = {}; }
  const title = typeof m.title === "string" && m.title ? m.title : "Clax";
  const tag = typeof m.tag === "string" && m.tag ? m.tag : undefined;
  e.waitUntil(self.registration.showNotification(title, { tag, renotify: !!tag, icon: "/_clax/app/icon-192.png", data: { url: shellPath(m.url) } }));
});

self.addEventListener("notificationclick", e => {
  e.notification.close();
  const url = new URL(shellPath(e.notification.data && e.notification.data.url), self.location.origin).href;
  e.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then(async all => {
    const open = all.find(c => c.url === url) || all.find(c => typeof c.navigate === "function");
    if (open) {
      await open.focus();
      if (open.url === url) return;
      // A window this worker does not control cannot be navigated.
      try { if (await open.navigate(url)) return; } catch { /* opened below */ }
    }
    await self.clients.openWindow(url);
  }));
});
