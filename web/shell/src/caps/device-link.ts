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
 * value or rejects with its coded error (a `CapError`). */
export const deviceLink: { call: ((cap: DeviceCap, method: string, args: unknown[]) => Promise<unknown>) | null } = { call: null };
