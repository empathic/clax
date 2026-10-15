// The way from the device capabilities (`notify`, `share`, `file`,
// `camera`, `location`; caps/device.ts) to the Clax app that hosts the view.
// The app host (`app/host.ts`) sets `call` in app mode, outside the inspect
// view, and clears it when it is uninstalled; anywhere else it stays null
// and the capabilities are not available (`use()` resolves null). Kept apart
// from caps/device.ts so the artifact entry carries only this.

export type DeviceCap = "notify" | "share" | "file" | "camera" | "location";

/** One call the app is asked to make. */
export type DeviceCall = { id: string; cap: DeviceCap; method: string; args: unknown[] };

/** `call` asks the app to make `cap.method(...args)`: it resolves the app's
 * value or rejects with its coded error (a `CapError`). `events` hears what
 * the app pushes for a running `location.watch`, as the app sent it (the
 * host calls each; caps/device.ts checks the shape): `{watch, kind:
 * "update", value}` or `{watch, kind: "end", error: {code, message}}`. */
export const deviceLink: {
  call: ((cap: DeviceCap, method: string, args: unknown[]) => Promise<unknown>) | null;
  events: Set<(e: unknown) => void>;
} = { call: null, events: new Set() };
