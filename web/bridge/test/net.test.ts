import { describe, expect, it, vi } from "vitest";
import { netLocals } from "../src/caps/net";

describe("net.fetch in the page", () => {
  it("sends the request to the shell and wraps the answer with ok, text() and json()", async () => {
    const call = vi.fn(async () => ({ url: "https://api.example.com/x", status: 200, headers: { "content-type": "application/json" }, body: "{\"a\":1}" }));
    const net = netLocals({ call } as never) as { fetch(u: unknown, i?: unknown): Promise<{ ok: boolean; status: number; url: string; headers: Record<string, string>; text(): Promise<string>; json(): Promise<unknown> }> };
    const res = await net.fetch("https://api.example.com/x", { method: "POST", headers: { a: "b" }, body: "{}" });
    expect(call).toHaveBeenCalledWith("net", "fetch", [{ url: "https://api.example.com/x", method: "POST", headers: { a: "b" }, body: "{}" }]);
    expect([res.ok, res.status, res.url, res.headers["content-type"]]).toEqual([true, 200, "https://api.example.com/x", "application/json"]);
    await expect(res.text()).resolves.toBe("{\"a\":1}");
    await expect(res.json()).resolves.toEqual({ a: 1 });
  });

  it("resolves error statuses, rejects json() on a body that is not JSON, and refuses a non-string URL", async () => {
    const call = vi.fn(async () => ({ url: "https://api.example.com/", status: 404, headers: {}, body: "nope" }));
    const net = netLocals({ call } as never) as { fetch(u: unknown, i?: unknown): Promise<{ ok: boolean; json(): Promise<unknown> }> };
    const res = await net.fetch("https://api.example.com/");
    expect(res.ok).toBe(false);
    await expect(res.json()).rejects.toBeInstanceOf(SyntaxError);
    await expect(net.fetch(7)).rejects.toMatchObject({ code: "invalid_argument" });
    expect(call).toHaveBeenCalledTimes(1);
  });
});

describe("net.fetch bodies", () => {
  type Res = { text(): Promise<string>; arrayBuffer(): Promise<ArrayBuffer>; blob(): Promise<Blob> };
  const fetchWith = async (answer: object) => {
    const net = netLocals({ call: async () => answer } as never) as { fetch(u: string): Promise<Res> };
    return net.fetch("https://tiles.example.com/1.png");
  };

  it("gives a base64 body's bytes as arrayBuffer() and a blob of the response's type", async () => {
    const res = await fetchWith({ url: "u", status: 200, headers: { "content-type": "image/png" }, body_base64: "iVBOR/8A" });
    expect([...new Uint8Array(await res.arrayBuffer())]).toEqual([0x89, 0x50, 0x4e, 0x47, 0xff, 0x00]);
    const blob = await res.blob();
    expect([blob.type, blob.size]).toEqual(["image/png", 6]);
    expect(await res.text()).toBe("�PNG�\u0000");
  });

  it("gives a text body's UTF-8 bytes as arrayBuffer()", async () => {
    const res = await fetchWith({ url: "u", status: 200, headers: {}, body: "é" });
    expect([...new Uint8Array(await res.arrayBuffer())]).toEqual([0xc3, 0xa9]);
    expect((await res.blob()).type).toBe("");
  });
});
