import { describe, expect, it, vi } from "vitest";
import { artifactLocals } from "../src/caps/artifact";
import { downloadsLocals } from "../src/caps/downloads";

const rpc = () => ({ call: vi.fn(async (_ns: string, _m: string, _args: unknown[]) => ({ version: "2" })) });

describe("artifact (page side)", () => {
  it("sends a complete document and refuses the rest before the shell", async () => {
    const r = rpc();
    const a = artifactLocals(r as never) as Record<string, (...x: unknown[]) => Promise<unknown>>;
    await expect(a.publish("﻿  <!DOCTYPE html><p>x")).resolves.toEqual({ version: "2" });
    await expect(a.publish("<p>fragment</p>")).rejects.toMatchObject({ code: "invalid_content" });
    await expect(a.publish({ "data/doc.json": "{}" })).rejects.toMatchObject({ code: "capability_disabled" });
    await expect(a.publish(7)).rejects.toMatchObject({ code: "invalid_content" });
    await expect(a.edit([])).rejects.toMatchObject({ code: "invalid_content" });
    await expect(a.sync(() => {})).rejects.toMatchObject({ code: "invalid_content" });
    expect(r.call).toHaveBeenCalledTimes(1);
  });
});

describe("downloads (page side)", () => {
  it("turns every accepted data form into a Blob", async () => {
    const r = rpc();
    const d = downloadsLocals(r as never) as { save(x: unknown): Promise<unknown> };
    for (const data of ["a,b", new Blob(["x"]), new Uint8Array([1, 2]).buffer, new Uint8Array([1, 2])]) {
      await d.save({ filename: "f.csv", data });
    }
    const blobs = r.call.mock.calls.map(c => (c[2] as unknown as [{ blob: Blob }])[0].blob);
    expect(blobs.every(b => b instanceof Blob)).toBe(true);
    expect(blobs.map(b => b.size)).toEqual([3, 1, 2, 2]);
  });

  it("transfers an ArrayBuffer (detached after the call) and copies views", async () => {
    const r = rpc();
    const d = downloadsLocals(r as never) as { save(x: unknown): Promise<unknown> };
    const buf = new Uint8Array([1, 2, 3]).buffer;
    await d.save({ filename: "f.csv", data: buf });
    expect(buf.byteLength).toBe(0);
    const view = new Uint8Array([4, 5]);
    await d.save({ filename: "f.csv", data: view });
    expect(view.byteLength).toBe(2);
    await expect(d.save({ filename: "f.csv", data: buf })).rejects.toMatchObject({ code: "bad_request" });
    expect(r.call).toHaveBeenCalledTimes(2);
  });

  it("refuses bad requests without asking the shell", async () => {
    const r = rpc();
    const d = downloadsLocals(r as never) as { save(x: unknown): Promise<unknown> };
    await expect(d.save(null)).rejects.toMatchObject({ code: "bad_request" });
    await expect(d.save({ filename: 7, data: "x" })).rejects.toMatchObject({ code: "bad_request" });
    await expect(d.save({ filename: "x".repeat(513), data: "x" })).rejects.toMatchObject({ code: "bad_request" });
    await expect(d.save({ filename: "a.txt", data: "" })).rejects.toMatchObject({ code: "bad_request" });
    await expect(d.save({ filename: "a.txt", data: 5 })).rejects.toMatchObject({ code: "bad_request" });
    await expect(d.save({ filename: "a.txt", data: "x", request: "tok" })).rejects.toMatchObject({ code: "request_unknown" });
    expect(r.call).not.toHaveBeenCalled();
  });
});
