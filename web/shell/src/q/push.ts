// Web Push on the owner's device (docs/contract.md, "Web Push"): the
// inbox's **Notify me on this device** asks the browser for permission from
// that click, subscribes the shell's service worker with the daemon's VAPID
// key (`GET /api/web-push`) and hands the subscription to the daemon
// (`POST /api/web-push/subscriptions`), which then pushes what waits on the
// owner to this device even while Clax is closed. **Turn off** unsubscribes
// and tells the daemon to forget it. On an iPhone, Web Push reaches only a
// web app on the Home Screen (iOS 16.4 and later): in Safari's tab the
// control says so instead.

/** Where the device stands: no Web Push here (`unsupported`), only from
 * the Home Screen (`install`), permission refused (`denied`), or off/on. */
export type PushState = "unsupported" | "install" | "denied" | "off" | "on";

/** What the controls use of the browser and the daemon (the defaults are
 * the real ones; tests pass their own). */
export type PushEnv = {
  nav: Navigator;
  N: typeof Notification | undefined;
  hasPushManager: boolean;
  /** `navigator.standalone` (iOS): false in a Safari tab, true from the Home Screen. */
  standalone: boolean | undefined;
  fetch: typeof fetch;
};

const realEnv = (): PushEnv => ({
  nav: navigator,
  N: typeof Notification === "undefined" ? undefined : Notification,
  hasPushManager: typeof window !== "undefined" && "PushManager" in window,
  standalone: (navigator as Navigator & { standalone?: boolean }).standalone,
  fetch: (...a) => fetch(...a),
});

/** An iPhone or iPad (iPadOS presents as a Mac with touch). */
const ios = (nav: Navigator) => /iPhone|iPad|iPod/.test(nav.userAgent) || (nav.platform === "MacIntel" && nav.maxTouchPoints > 1);

/** The bytes of base64url `s` (the daemon's VAPID public key). */
export function keyBytes(s: string): Uint8Array<ArrayBuffer> {
  const b = atob(s.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (s.length % 4)) % 4));
  const out = new Uint8Array(new ArrayBuffer(b.length));
  for (let i = 0; i < b.length; i++) out[i] = b.charCodeAt(i);
  return out;
}

async function ok(res: Response): Promise<Response> {
  if (res.ok) return res;
  let msg = res.statusText;
  try { msg = (await res.json()).error?.message ?? msg; } catch { /* not JSON */ }
  throw new Error(msg);
}

/** Where this device stands now: whether it can push, and whether its
 * service worker holds a subscription. */
export async function pushState(env: PushEnv = realEnv()): Promise<PushState> {
  const sw = env.nav.serviceWorker;
  if (!sw || !env.hasPushManager || !env.N) return ios(env.nav) && env.standalone === false ? "install" : "unsupported";
  if (env.N.permission === "denied") return "denied";
  const reg = await sw.getRegistration("/");
  const sub = await reg?.pushManager.getSubscription();
  return sub && env.N.permission === "granted" ? "on" : "off";
}

/** Turns push on for this device. Call it from a click: the permission
 * prompt needs one. Resolves to the state it leaves the device in. */
export async function turnOn(env: PushEnv = realEnv()): Promise<PushState> {
  const sw = env.nav.serviceWorker;
  if (!sw || !env.hasPushManager || !env.N) return "unsupported";
  const perm = await env.N.requestPermission();
  if (perm === "denied") return "denied";
  if (perm !== "granted") return "off";
  const { public_key } = await (await ok(await env.fetch("/api/web-push"))).json() as { public_key: string };
  const reg = (await sw.getRegistration("/")) ?? (await sw.register("/sw.js", { scope: "/" }));
  await sw.ready;
  let sub = await reg.pushManager.getSubscription();
  // A subscription made with another key (a home made again) is replaced.
  const key = keyBytes(public_key);
  const held = sub?.options.applicationServerKey;
  if (sub && held && !sameBytes(new Uint8Array(held), key)) { await sub.unsubscribe(); sub = null; }
  sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  await ok(await env.fetch("/api/web-push/subscriptions", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(sub.toJSON()) }));
  return "on";
}

/** Turns push off for this device: the daemon forgets it, and its
 * subscription ends. */
export async function turnOff(env: PushEnv = realEnv()): Promise<PushState> {
  const reg = await env.nav.serviceWorker?.getRegistration("/");
  const sub = await reg?.pushManager.getSubscription();
  if (sub) {
    await ok(await env.fetch("/api/web-push/subscriptions", { method: "DELETE", headers: { "content-type": "application/json" }, body: JSON.stringify({ endpoint: sub.endpoint }) }));
    await sub.unsubscribe();
  }
  return "off";
}

const sameBytes = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);
