// The device capabilities in the Clax app (contract: clax-extensions.d.ts,
// `ClaxExtensions.Device`), loaded on a page's first call: each call is
// forwarded to the app through the app host (device-link.ts, app/host.ts),
// which asks the person once per app and capability and answers with a
// value or a coded error (`not_granted`, `cancelled`, `unavailable`,
// `bad_request`, `too_large`). The shell checks the method name, that the
// arguments are plain data, and that a call that acts came from the viewer's
// gesture in the page (`frameGesture`, gesture.ts; `no_gesture`
// otherwise); the app checks the arguments' shape.
//
// `location.watch({watch, minDistance})` starts a watch under the page's
// own ID; the shell gives the app an ID of its own (random per load), and
// passes the app's pushes for it (`deviceEvent`) to the page as
// `location`/`update` and `location`/`end` events under the page's ID.
// `location.unwatch({watch})` stops one (no gesture needed). When the
// frame's document leaves, every watch it started is stopped.
//
// A location call spends the person's tap: `location.current` and
// `location.watch` each need a press on the frame (attested by the app,
// `hostArrival.presses`) newer than the one the last of them used, and newer
// than the end of a watch (the person stopping it with the app's pill, or
// leaving the app), so one tap starts at most one.
import { type DeviceCap, deviceLink } from "./device-link";
import { CapError } from "./errors";
import { frameGesture, hostArrival } from "./gesture";
import type { HandlerFactory } from "./host";

export const DEVICE_METHODS: Record<DeviceCap, readonly string[]> = {
  notify: ["schedule", "cancel", "list"],
  share: ["text", "file"],
  file: ["pick", "save"],
  camera: ["take", "pick"],
  location: ["current", "watch", "unwatch"],
};

/** Calls that need no gesture: reading and cancelling reminders, and
 * stopping a watch. */
const NO_GESTURE = new Set(["notify.list", "notify.cancel", "location.unwatch"]);

const watchPrefix = Array.from(crypto.getRandomValues(new Uint8Array(4)), b => b.toString(16).padStart(2, "0")).join("");
let watchSeq = 0;

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
  return env => {
    // The running watches: the app's ID → the page's.
    const watches = new Map<string, string>();
    // The attested press the last location call (or a watch's end) used up.
    let spent = -1;
    const onEvent = (a: unknown) => {
      const e = (a && typeof a === "object" ? a : {}) as { watch?: unknown; kind?: unknown; value?: unknown; error?: { code?: unknown; message?: unknown } };
      const page = typeof e.watch === "string" ? watches.get(e.watch) : undefined;
      if (page === undefined) return;
      if (e.kind === "update") env.post({ type: "clax:event", ns: "location", topic: "update", data: { watch: page, value: e.value } });
      else if (e.kind === "end") {
        watches.delete(e.watch as string);
        spent = hostArrival.presses;
        const code = e.error?.code, message = e.error?.message;
        env.post({ type: "clax:event", ns: "location", topic: "end", data: { watch: page, code: typeof code === "string" ? code : "unavailable", message: typeof message === "string" ? message : "location stopped" } });
      }
    };
    if (cap === "location") deviceLink.events.add(onEvent);
    /** Stops every watch this document started; the page hears nothing. */
    const stopAll = () => {
      for (const id of watches.keys()) void deviceLink.call?.("location", "unwatch", [{ watch: id }]).catch(() => {});
      watches.clear();
    };
    return {
      call(method, args) {
        if (!DEVICE_METHODS[cap].includes(method)) return Promise.reject(new CapError("capability_removed", `${cap}.${String(method)} is not part of this runtime`));
        // Everything else acts on the device or opens a system sheet: only
        // from the viewer's tap or key in the page.
        if (!NO_GESTURE.has(`${cap}.${method}`) && !frameGesture()) {
          return Promise.reject(new CapError("no_gesture", `${cap}.${method} needs the person's tap or key press in the page; call it from a click handler`));
        }
        if (!plain(args)) return Promise.reject(new CapError("bad_request", `${cap}.${method} takes plain data (strings, numbers, objects)`));
        const call = deviceLink.call;
        if (!call) return Promise.reject(new CapError("unavailable", `${cap} is available only in the Clax app`));
        if (cap === "location" && method !== "unwatch") {
          if (hostArrival.presses <= spent) return Promise.reject(new CapError("no_gesture", `location.${method} needs a new tap: one tap starts one location call`));
          spent = hostArrival.presses;
        }
        if (method !== "watch" && method !== "unwatch") return call(cap, method, args);
        const a = args[0] as { watch?: unknown; minDistance?: unknown } | undefined;
        const page = a?.watch;
        if (typeof page !== "string") return Promise.reject(new CapError("bad_request", `location.${method} takes {watch}`));
        if (method === "unwatch") {
          const id = [...watches].find(([, p]) => p === page)?.[0];
          if (id === undefined) return Promise.resolve({ stopped: false });
          watches.delete(id);
          return call(cap, method, [{ watch: id }]);
        }
        const id = `${watchPrefix}-${++watchSeq}`;
        watches.set(id, page);
        return call(cap, method, [{ watch: id, minDistance: a?.minDistance }]).catch((e: unknown) => {
          watches.delete(id);
          throw e;
        });
      },
      leave: stopAll,
      reset: stopAll,
      dispose() {
        stopAll();
        deviceLink.events.delete(onEvent);
      },
    };
  };
}
