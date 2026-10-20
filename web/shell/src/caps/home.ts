// The `home` capability in the Clax app (contract: clax-extensions.d.ts,
// `ClaxExtensions.Home`), loaded on a page's first call: what the home
// screen shows and the screens it opens, answered by the app only for its
// home app (CapabilityHost asks `home.available` before granting it, and
// the app refuses every call from any other app). Reads (`apps`, `makes`,
// `unread`, `needsKey`, `chrome`) need no gesture; every call that opens something
// must come from the viewer's tap or key in the page (`no_gesture`
// otherwise); `shown` and `tiles` need none either. Positional arguments go
// to the app as one object: `open(id)` and `openMake(id)` as `{id}`,
// `menu(id, rect)` as `{id, rect: {x, y, w, h}}`, `menu(id, rect, {items})`
// as `{id, rect, items}` (at most MAX_MENU_ITEMS, labels trimmed, at most
// MAX_LABEL characters) and as `{id, rect}` without items; it resolves the
// picked item's ID, or null. `tiles(list)` goes as `{tiles: [{id, rect}]}`
// (at most MAX_TILES). The app's changes (`deviceEvent({cap: "home", name, data})`, name
// `apps`, `makes`, `unread`, `key` or `chrome`) reach the page as `home`/<name>
// events once the handler is loaded.
import { deviceLink } from "./device-link";
import { CapError } from "./errors";
import { frameGesture } from "./gesture";
import type { HandlerFactory } from "./host";

export const HOME_METHODS = ["apps", "makes", "unread", "needsKey", "open", "openMake", "newApp", "menu", "openInbox", "openSettings", "importApp", "shown", "tiles", "chrome"] as const;

const READS = new Set<string>(["apps", "makes", "unread", "needsKey", "shown", "tiles", "chrome"]);

/** The most tiles one `tiles` call may give. */
export const MAX_TILES = 512;
export const HOME_EVENTS = new Set<string>(["apps", "makes", "unread", "key", "chrome"]);

const id = (method: string, v: unknown): string => {
  if (typeof v !== "string" || v.length === 0 || v.length > 64) throw new CapError("bad_request", `home.${method} takes an app ID`);
  return v;
};

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

type Rect = { x: number; y: number; w: number; h: number };

/** `v` as a rect, or null when it is not one. */
function rectOf(v: unknown): Rect | null {
  const r = (v ?? null) as { x?: unknown; y?: unknown; w?: unknown; h?: unknown } | null;
  if (!r || typeof r !== "object" || !finite(r.x) || !finite(r.y) || !finite(r.w) || !finite(r.h) || r.w < 0 || r.h < 0) return null;
  return { x: r.x, y: r.y, w: r.w, h: r.h };
}

/** The most items a page adds to a menu, and the longest label. */
export const MAX_MENU_ITEMS = 4;
export const MAX_LABEL = 32;

type MenuItem = { id: string; label: string };

const badItems = () => new CapError("bad_request", `home.menu takes up to ${MAX_MENU_ITEMS} items {id, label}: IDs of 1 to 64 characters, each once, and labels of 1 to ${MAX_LABEL} characters of plain text`);

const plain = (s: string) => [...s].every(ch => { const c = ch.codePointAt(0)!; return c >= 0x20 && c !== 0x7f; });

/** The items `menu`'s options add, or undefined for none. */
function itemsOf(options: unknown): MenuItem[] | undefined {
  if (options === undefined || options === null) return undefined;
  if (typeof options !== "object") throw badItems();
  const list = (options as { items?: unknown }).items;
  if (list === undefined) return undefined;
  if (!Array.isArray(list) || list.length > MAX_MENU_ITEMS) throw badItems();
  const seen = new Set<string>();
  return list.map(i => {
    const e = (i ?? null) as { id?: unknown; label?: unknown } | null;
    const label = e && typeof e.label === "string" ? e.label.trim() : "";
    if (!e || typeof e.id !== "string" || e.id.length === 0 || e.id.length > 64 || seen.has(e.id)) throw badItems();
    if (!label || [...label].length > MAX_LABEL || !plain(label)) throw badItems();
    seen.add(e.id);
    return { id: e.id, label };
  });
}

/** The one argument object the app takes for `method`. */
function argsFor(method: string, args: unknown[]): unknown[] {
  switch (method) {
    case "open": case "openMake": return [{ id: id(method, args[0]) }];
    case "menu": {
      const rect = rectOf(args[1]);
      if (!rect) throw new CapError("bad_request", "home.menu takes an app ID and the tile's rect {x, y, w, h}");
      const items = itemsOf(args[2]);
      return [items ? { id: id(method, args[0]), rect, items } : { id: id(method, args[0]), rect }];
    }
    case "tiles": {
      const list = args[0];
      const bad = () => new CapError("bad_request", `home.tiles takes a list of at most ${MAX_TILES} {id, rect: {x, y, w, h}}`);
      if (!Array.isArray(list) || list.length > MAX_TILES) throw bad();
      return [{
        tiles: list.map(t => {
          const e = (t ?? null) as { id?: unknown; rect?: unknown } | null;
          const rect = e && typeof e === "object" ? rectOf(e.rect) : null;
          if (!rect) throw bad();
          return { id: id(method, e!.id), rect };
        }),
      }];
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
    call(method, args, gesture) {
      if (!(HOME_METHODS as readonly string[]).includes(method)) return Promise.reject(new CapError("capability_removed", `home.${String(method)} is not part of this runtime`));
      if (!READS.has(method) && !(gesture?.composer ?? frameGesture())) {
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
