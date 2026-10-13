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
