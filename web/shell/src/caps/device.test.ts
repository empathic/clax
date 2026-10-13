import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { type DeviceCall, answerDevice, callDevice, deviceLinked, linkDevice } from "./device-link";
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

const unlinks: (() => void)[] = [];
afterEach(() => { for (const u of unlinks.splice(0)) u(); });
function link() {
  const sent: DeviceCall[] = [];
  unlinks.push(linkDevice(c => sent.push(c)));
  return sent;
}

describe("the device capabilities", () => {
  it("resolve null outside the Clax app, even when declared", async () => {
    expect(deviceLinked()).toBe(false);
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
    expect(sent[0]).toEqual({ id: expect.stringMatching(/^d\d+$/), cap: "notify", method: "schedule", args: [at] });
    answerDevice({ id: sent[0].id, ok: true, value: { id: "water" } });
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
    answerDevice({ id: "d-unknown", ok: true, value: 1 });
    answerDevice({ id: sent[0].id, ok: false, error: { code: "not_granted", message: "the person did not allow location" } });
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

  it("rejects calls still waiting when the app unlinks", async () => {
    const unlink = linkDevice(() => {});
    const p = callDevice("camera", "take", []);
    unlink();
    await expect(p).rejects.toMatchObject({ code: "unavailable" });
    expect(deviceLinked()).toBe(false);
    await expect(callDevice("camera", "take", [])).rejects.toMatchObject({ code: "unavailable" });
  });
});
