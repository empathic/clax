// The `home` capability in the Clax app (contract: clax-extensions.d.ts,
// `ClaxExtensions.Home`), loaded on a page's first call: what the home
// screen shows and the screens it opens, answered by the app only for its
// home app (CapabilityHost asks `home.available` before granting it, and
// the app refuses every call from any other app). Reads (`apps`, `makes`,
// `unread`, `needsKey`) need no gesture; every call that opens something
// must come from the viewer's tap or key in the page (`no_gesture`
// otherwise); `shown` needs none either. Positional arguments go to the app as one object: `open(id)`
// and `openMake(id)` as `{id}`, `menu(id, rect)` as `{id, rect: {x, y, w,
// h}}`. The app's changes (`deviceEvent({cap: "home", name, data})`, name
// `apps`, `makes`, `unread` or `key`) reach the page as `home`/<name>
// events once the handler is loaded.
import { deviceLink } from "./device-link";
import { CapError } from "./errors";
import { frameGesture } from "./gesture";
import type { HandlerFactory } from "./host";

export const HOME_METHODS = ["apps", "makes", "unread", "needsKey", "open", "openMake", "newApp", "menu", "openInbox", "openSettings", "importApp", "shown"] as const;

const READS = new Set<string>(["apps", "makes", "unread", "needsKey", "shown"]);
export const HOME_EVENTS = new Set<string>(["apps", "makes", "unread", "key"]);

const id = (method: string, v: unknown): string => {
  if (typeof v !== "string" || v.length === 0 || v.length > 64) throw new CapError("bad_request", `home.${method} takes an app ID`);
  return v;
};

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** The one argument object the app takes for `method`. */
function argsFor(method: string, args: unknown[]): unknown[] {
  switch (method) {
    case "open": case "openMake": return [{ id: id(method, args[0]) }];
    case "menu": {
      const r = (args[1] ?? null) as { x?: unknown; y?: unknown; w?: unknown; h?: unknown } | null;
      if (!r || typeof r !== "object" || !finite(r.x) || !finite(r.y) || !finite(r.w) || !finite(r.h) || r.w < 0 || r.h < 0) {
        throw new CapError("bad_request", "home.menu takes an app ID and the tile's rect {x, y, w, h}");
      }
      return [{ id: id(method, args[0]), rect: { x: r.x, y: r.y, w: r.w, h: r.h } }];
    }
    default: return [];
  }
}

export const homeHandler: HandlerFactory = env => {
  const onEvent = (a: unknown) => {
    const e = (a && typeof a === "object" ? a : {}) as { cap?: unknown; name?: unknown; data?: unknown };
    if (e.cap !== "home" || typeof e.name !== "string" || !HOME_EVENTS.has(e.name)) return;
    env.post({ type: "clax:event", ns: "home", topic: e.name, data: e.data });
  };
  deviceLink.events.add(onEvent);
  return {
    call(method, args) {
      if (!(HOME_METHODS as readonly string[]).includes(method)) return Promise.reject(new CapError("capability_removed", `home.${String(method)} is not part of this runtime`));
      if (!READS.has(method) && !frameGesture()) {
        return Promise.reject(new CapError("no_gesture", `home.${method} needs the person's tap or key press in the page; call it from a click handler`));
      }
      const call = deviceLink.call;
      if (!call) return Promise.reject(new CapError("unavailable", "home is available only in the Clax app"));
      try {
        return call("home", method, argsFor(method, args));
      } catch (e) {
        return Promise.reject(e);
      }
    },
    dispose() {
      deviceLink.events.delete(onEvent);
    },
  };
};
