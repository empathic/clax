import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { type DeviceCall, deviceLink } from "./device-link";
import { CapError } from "./errors";
import { CapabilityHost, type CapEnv } from "./host";
import { REGISTRY } from "./registry";

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

afterEach(() => { deviceLink.call = null; });
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
    const { e, posted } = env({ notify: {}, share: {}, files: {}, camera: {}, location: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    for (const name of ["notify", "share", "files", "camera", "location"]) await host.handle({ type: "clax:use", id: name, name });
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
});
