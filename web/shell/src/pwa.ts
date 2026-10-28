// The installed web app (web/shell/public/_clax/app.webmanifest): the
// shell registers its service worker (`/sw.js`, scope `/`), which caches
// nothing and handles no fetch, so Add to Home Screen opens Clax as an app
// and Web Push can reach it there.

/** Where the daemon serves the service worker, and its scope. */
export const SW_URL = "/sw.js";
export const SW_SCOPE = "/";

/** Registers the shell's service worker where the browser has them (a
 * secure context: https, or localhost); a refusal is ignored, the shell
 * works the same without it. */
export function registerServiceWorker(nav: Navigator = navigator): void {
  const sw = nav.serviceWorker;
  if (!sw) return;
  sw.register(SW_URL, { scope: SW_SCOPE }).catch(() => {});
}
