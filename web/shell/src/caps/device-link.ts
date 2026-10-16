// The way from the device capabilities (`notify`, `share`, `file`,
// `camera`, `location`; caps/device.ts) and `home` (caps/home.ts) to the
// Clax app that hosts the view.
// The app host (`app/host.ts`) sets `call` in app mode, outside the inspect
// view, and clears it when it is uninstalled; anywhere else it stays null
// and the capabilities are not available (`use()` resolves null). Kept apart
// from caps/device.ts so the artifact entry carries only this.

export type DeviceCap = "notify" | "share" | "file" | "camera" | "location";

/** A capability the app answers on this way: the device ones and `home`. */
export type LinkedCap = DeviceCap | "home";

/** One call the app is asked to make. */
export type DeviceCall = { id: string; cap: LinkedCap; method: string; args: unknown[] };

/** `home` asks the app, once per view, whether this view is its home app
 * (`home.available`); a failed answer is no. Set with `call`.
 *
 * `call` asks the app to make `cap.method(...args)`: it resolves the app's
 * value or rejects with its coded error (a `CapError`). `events` hears what
 * the app pushes, as the app sent it (the host calls each; the handlers
 * check the shape): for a running `location.watch` `{watch, kind:
 * "update", value}` or `{watch, kind: "end", error: {code, message}}`, and
 * for `home` `{cap: "home", name, data}`. */
export const deviceLink: {
  call: ((cap: LinkedCap, method: string, args: unknown[]) => Promise<unknown>) | null;
  home?: (() => Promise<boolean>) | undefined;
  events: Set<(e: unknown) => void>;
} = { call: null, events: new Set() };
