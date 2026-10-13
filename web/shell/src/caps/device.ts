// The device capabilities in the Clax app (contract: clax-extensions.d.ts,
// `ClaxExtensions.Device`), loaded on a page's first call: each call is
// forwarded to the app through the app host (device-link.ts, app/host.ts),
// which asks the person once per app and capability and answers with a
// value or a coded error (`not_granted`, `cancelled`, `unavailable`,
// `bad_request`, `too_large`). The shell checks only the method name and
// that the arguments are plain data; the app checks their shape.
import { type DeviceCap, deviceLink } from "./device-link";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export const DEVICE_METHODS: Record<DeviceCap, readonly string[]> = {
  notify: ["schedule", "cancel", "list"],
  share: ["text", "file"],
  files: ["pick", "save"],
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
      if (!plain(args)) return Promise.reject(new CapError("bad_request", `${cap}.${method} takes plain data (strings, numbers, objects)`));
      const call = deviceLink.call;
      if (!call) return Promise.reject(new CapError("unavailable", `${cap} is available only in the Clax app`));
      return call(cap, method, args);
    },
  });
}
