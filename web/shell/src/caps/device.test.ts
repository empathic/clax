import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { type DeviceCall, deviceLink } from "./device-link";
import { CapError } from "./errors";
import { CapabilityHost, type CapEnv } from "./host";
import { hostArrival } from "./gesture";
import { REGISTRY } from "./registry";

/** Whether the viewer's gesture in the page holds, as `frameGesture` says. */
let gesture = true;
vi.mock("./gesture", async orig => ({ ...(await orig<typeof import("./gesture")>()), frameGesture: () => gesture }));

function env(declared: CapEnv["declared"]) {
  const posted: ShellToBridge[] = [];
  const e: CapEnv = {
    aid: "7q3k9mzx2b4t", version: 1, pinned: false, token: "t",
    viewer: async () => ({ publicId: "u_00000000000000000000aa", name: "Alex" }),
    declared,
    prompt: vi.fn(async () => "allow" as const),
    post: m => posted.push(m),
    reload: vi.fn(),
  };
  return { e, posted };
}

afterEach(() => { deviceLink.call = null; gesture = true; });
/** Links a fake app: each call is kept, with its answer's resolve and reject. */
function link() {
  const sent: (Omit<DeviceCall, "id"> & { resolve(v: unknown): void; reject(e: unknown): void })[] = [];
  deviceLink.call = (cap, method, args) => new Promise((resolve, reject) => { sent.push({ cap, method, args, resolve, reject }); });
  return sent;
}

describe("the device capabilities", () => {
  it("resolve null outside the Clax app, even when declared", async () => {
    expect(deviceLink.call).toBeNull();
    const { e, posted } = env({ notify: {}, camera: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:use", id: "u1", name: "notify" });
    await host.handle({ type: "clax:call", id: "c1", ns: "camera", method: "take", args: [] });
    expect(posted[0]).toMatchObject({ granted: false, config: null });
    expect(posted[1]).toMatchObject({ ok: false, error: { code: "not_granted" } });
  });

  it("in the app, are granted when declared and forward each call with a request ID", async () => {
    const sent = link();
    const { e, posted } = env({ notify: {}, share: {}, file: {}, camera: {}, location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    for (const name of ["notify", "share", "file", "camera", "location"]) await host.handle({ type: "clax:use", id: name, name });
    expect(posted.map(m => (m as { granted: boolean }).granted)).toEqual([true, true, true, true, true]);
    const at = { id: "water", title: "Water", body: "Drink", at: "2026-10-13T09:00:00Z", route: "#/plants" };
    const done = host.handle({ type: "clax:call", id: "c1", ns: "notify", method: "schedule", args: [at] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ cap: "notify", method: "schedule", args: [at] });
    sent[0].resolve({ id: "water" });
    await done;
    expect(posted.at(-1)).toEqual({ type: "clax:call-result", id: "c1", ok: true, value: { id: "water" } });
  });

  it("is not granted when undeclared, in the app too", async () => {
    link();
    const { e, posted } = env({ db: {} });
    await new CapabilityHost(Promise.resolve(e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "location" });
    expect(posted[0]).toMatchObject({ granted: false });
  });

  it("passes the app's coded errors to the page", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const done = host.handle({ type: "clax:call", id: "c1", ns: "location", method: "current", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    sent[0].reject(new CapError("not_granted", "the person did not allow location"));
    await done;
    expect(posted.at(-1)).toEqual({ type: "clax:call-result", id: "c1", ok: false, error: { code: "not_granted", message: "the person did not allow location" } });
  });

  it("refuses an unknown method and arguments that are not plain data, asking the app nothing", async () => {
    const sent = link();
    const { e, posted } = env({ share: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:call", id: "c1", ns: "share", method: "url", args: [] });
    await host.handle({ type: "clax:call", id: "c2", ns: "share", method: "text", args: [new Blob(["x"])] });
    expect(posted.map(m => (m as { error: { code: string } }).error.code)).toEqual(["capability_removed", "bad_request"]);
    expect(sent).toEqual([]);
  });

  it("refuses a call that acts without the viewer's gesture, but lists and cancels reminders without one", async () => {
    const sent = link();
    gesture = false;
    const { e, posted } = env({ notify: {}, camera: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:call", id: "c1", ns: "camera", method: "take", args: [] });
    await host.handle({ type: "clax:call", id: "c2", ns: "notify", method: "schedule", args: [{ id: "a" }] });
    expect(posted.map(m => (m as { error?: { code: string } }).error?.code)).toEqual(["no_gesture", "no_gesture"]);
    const list = host.handle({ type: "clax:call", id: "c3", ns: "notify", method: "list", args: [] });
    const cancel = host.handle({ type: "clax:call", id: "c4", ns: "notify", method: "cancel", args: ["a"] });
    await vi.waitFor(() => expect(sent.map(c => c.method)).toEqual(["list", "cancel"]));
    sent[0].resolve([]);
    sent[1].resolve({ cancelled: false });
    await Promise.all([list, cancel]);
  });

  it("location.watch gives the app an ID of its own, passes its news to the page under the page's ID, and unwatch needs no gesture", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const started = host.handle({ type: "clax:call", id: "c1", ns: "location", method: "watch", args: [{ watch: "w1", minDistance: 20 }] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    const appId = (sent[0].args[0] as { watch: string }).watch;
    expect(appId).not.toBe("w1");
    expect(sent[0].args[0]).toEqual({ watch: appId, minDistance: 20 });
    sent[0].resolve({ watching: true });
    await started;
    for (const f of deviceLink.events) f({ watch: appId, kind: "update", value: { lat: 1, lon: 2, accuracy: 5, precise: true } });
    for (const f of deviceLink.events) f({ watch: "someone-else", kind: "update", value: {} });
    for (const f of deviceLink.events) { f(null); f({ watch: 7, kind: "update" }); f({ watch: appId, kind: "other" }); }
    expect(posted.at(-1)).toEqual({ type: "clax:event", ns: "location", topic: "update", data: { watch: "w1", value: { lat: 1, lon: 2, accuracy: 5, precise: true } } });
    gesture = false;
    const stop = host.handle({ type: "clax:call", id: "c2", ns: "location", method: "unwatch", args: [{ watch: "w1" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]).toMatchObject({ method: "unwatch", args: [{ watch: appId }] });
    sent[1].resolve({ stopped: true });
    await stop;
    // Gone: its news no longer reaches the page, and starting one needs a tap.
    const before = posted.length;
    for (const f of deviceLink.events) f({ watch: appId, kind: "update", value: {} });
    await host.handle({ type: "clax:call", id: "c3", ns: "location", method: "watch", args: [{ watch: "w2" }] });
    expect(posted.slice(before)).toEqual([{ type: "clax:call-result", id: "c3", ok: false, error: { code: "no_gesture", message: expect.any(String) } }]);
    host.dispose();
  });

  it("an ended watch tells the page once; the document leaving stops the rest", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    for (const [i, w] of ["a", "b"].entries()) {
      hostArrival.presses++;
      const p = host.handle({ type: "clax:call", id: w, ns: "location", method: "watch", args: [{ watch: w }] });
      await vi.waitFor(() => expect(sent).toHaveLength(i + 1));
      sent.at(-1)!.resolve({ watching: true });
      await p;
    }
    const [idA, idB] = sent.map(c => (c.args[0] as { watch: string }).watch);
    for (const f of deviceLink.events) f({ watch: idA, kind: "end", error: { code: "stopped", message: "left the screen" } });
    for (const f of deviceLink.events) f({ watch: idA, kind: "end", error: { code: "stopped", message: "again" } });
    expect(posted.filter(m => m.type === "clax:event")).toEqual([{ type: "clax:event", ns: "location", topic: "end", data: { watch: "a", code: "stopped", message: "left the screen" } }]);
    const listeners = deviceLink.events.size;
    host.dispose();
    expect(sent.slice(2)).toEqual([expect.objectContaining({ method: "unwatch", args: [{ watch: idB }] })]);
    expect(deviceLink.events.size).toBe(listeners - 1);
  });

  it("a location call spends the tap: one press starts one, and after a watch ends a new press is needed", async () => {
    const sent = link();
    const { e, posted } = env({ location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const codes = () => posted.filter(m => m.type === "clax:call-result" && !m.ok).map(m => (m as { error: { code: string } }).error.code);
    hostArrival.presses++;
    const started = host.handle({ type: "clax:call", id: "c1", ns: "location", method: "watch", args: [{ watch: "w1" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    // The same tap: neither a second watch nor a one-shot call.
    await host.handle({ type: "clax:call", id: "c2", ns: "location", method: "watch", args: [{ watch: "w2" }] });
    await host.handle({ type: "clax:call", id: "c3", ns: "location", method: "current", args: [] });
    expect(codes()).toEqual(["no_gesture", "no_gesture"]);
    sent[0].resolve({ watching: true });
    await started;
    // The person taps the page again, then stops the watch with the pill.
    hostArrival.presses++;
    const appId = (sent[0].args[0] as { watch: string }).watch;
    for (const f of deviceLink.events) f({ watch: appId, kind: "end", error: { code: "stopped", message: "stopped" } });
    await host.handle({ type: "clax:call", id: "c4", ns: "location", method: "watch", args: [{ watch: "w3" }] });
    expect(codes()).toEqual(["no_gesture", "no_gesture", "no_gesture"]);
    // A tap after the stop starts it again.
    hostArrival.presses++;
    const again = host.handle({ type: "clax:call", id: "c5", ns: "location", method: "watch", args: [{ watch: "w4" }] });
    await vi.waitFor(() => expect(sent).toHaveLength(2));
    sent[1].resolve({ watching: true });
    await again;
    host.dispose();
  });
});
