// The way from the device capabilities (`notify`, `share`, `files`, `camera`,
// `location`; caps/device.ts) to the Clax app that hosts the view. The app
// host (`app/host.ts`) links itself here in app mode, outside the inspect
// view; anywhere else nothing is linked and the capabilities are not
// available (`use()` resolves null). Each call gets a request ID; the app
// answers it through `claxHost.deviceResult`, with a value or a coded error
// (`not_granted`, `cancelled`, `unavailable`, `bad_request`, `too_large`).
import { CapError } from "./errors";

export const DEVICE_CAPS = ["notify", "share", "files", "camera", "location"] as const;
export type DeviceCap = (typeof DEVICE_CAPS)[number];

/** One call the app is asked to make. */
export type DeviceCall = { id: string; cap: DeviceCap; method: string; args: unknown[] };

type Pending = { resolve(v: unknown): void; reject(e: CapError): void };

let sender: ((c: DeviceCall) => void) | null = null;
const pending = new Map<string, Pending>();
let seq = 0;

export function isDeviceCap(name: string): name is DeviceCap {
  return (DEVICE_CAPS as readonly string[]).includes(name);
}

/** Whether an app host is linked: the device capabilities are available. */
export function deviceLinked(): boolean {
  return sender !== null;
}

/** Links the app host's `send`; returns the unlink, which rejects every
 * call still waiting `unavailable`. */
export function linkDevice(send: (c: DeviceCall) => void): () => void {
  sender = send;
  return () => {
    if (sender !== send) return;
    sender = null;
    for (const [id, p] of pending) {
      pending.delete(id);
      p.reject(new CapError("unavailable", "the app closed this view"));
    }
  };
}

/** Asks the app to make `cap.method(...args)`; resolves its value, or
 * rejects with its coded error. */
export function callDevice(cap: DeviceCap, method: string, args: unknown[]): Promise<unknown> {
  const send = sender;
  if (!send) return Promise.reject(new CapError("unavailable", `${cap} is available only in the Clax app`));
  const id = `d${++seq}`;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    send({ id, cap, method, args });
  });
}

/** The app's answer `a` (`{id, ok: true, value}` or `{id, ok: false,
 * error: {code, message}}`); anything else, or an ID no call waits on, is
 * dropped. */
export function answerDevice(a: unknown): void {
  if (!a || typeof a !== "object") return;
  const { id, ok, value, error } = a as { id?: unknown; ok?: unknown; value?: unknown; error?: unknown };
  if (typeof id !== "string") return;
  const p = pending.get(id);
  if (!p) return;
  pending.delete(id);
  if (ok === true) { p.resolve(value ?? null); return; }
  const e = (error && typeof error === "object" ? error : {}) as { code?: unknown; message?: unknown };
  p.reject(new CapError(typeof e.code === "string" ? e.code : "unavailable", typeof e.message === "string" ? e.message : "the device call failed"));
}
