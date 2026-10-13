// The device capabilities in the Clax app (contract: clax-extensions.d.ts,
// `ClaxExtensions.Device`), loaded on a page's first call: each call is
// forwarded to the app through the app host (device-link.ts, app/host.ts),
// which asks the person once per app and capability and answers with a
// value or a coded error (`not_granted`, `cancelled`, `unavailable`,
// `bad_request`, `too_large`). The shell checks the method name, that the
// arguments are plain data, and that a call that acts came from the viewer's
// gesture in the page (`frameGesture`, gesture.ts; `no_gesture`
// otherwise); the app checks the arguments' shape.
import { type DeviceCap, deviceLink } from "./device-link";
import { CapError } from "./errors";
import { frameGesture } from "./gesture";
import type { HandlerFactory } from "./host";

export const DEVICE_METHODS: Record<DeviceCap, readonly string[]> = {
  notify: ["schedule", "cancel", "list"],
  share: ["text", "file"],
  file: ["pick", "save"],
  camera: ["take", "pick"],
  location: ["current"],
};

/** Whether `v` survives JSON as it is (the app's channel is JSON). */
function plain(v: unknown, depth = 0): boolean {
  if (depth > 8) return false;
  if (v === null || typeof v === "string" || typeof v === "boolean") return true;
  if (typeof v === "number") return Number.isFinite(v);
  if (Array.isArray(v)) return v.every(x => plain(x, depth + 1));
  if (typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype) return Object.values(v as object).every(x => x === undefined || plain(x, depth + 1));
  return false;
}

export function deviceHandler(cap: DeviceCap): HandlerFactory {
  return () => ({
    call(method, args) {
      if (!DEVICE_METHODS[cap].includes(method)) return Promise.reject(new CapError("capability_removed", `${cap}.${String(method)} is not part of this runtime`));
      // Everything but reading and cancelling reminders acts on the device
      // or opens a system sheet: only from the viewer's tap or key in the page.
      if (!(cap === "notify" && (method === "list" || method === "cancel")) && !frameGesture()) {
        return Promise.reject(new CapError("no_gesture", `${cap}.${method} needs the person's tap or key press in the page; call it from a click handler`));
      }
      if (!plain(args)) return Promise.reject(new CapError("bad_request", `${cap}.${method} takes plain data (strings, numbers, objects)`));
      const call = deviceLink.call;
      if (!call) return Promise.reject(new CapError("unavailable", `${cap} is available only in the Clax app`));
      return call(cap, method, args);
    },
  });
}
