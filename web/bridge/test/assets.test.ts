import { describe, expect, it, vi } from "vitest";
import { assetsLocals } from "../src/caps/assets";

describe("assets (page side)", () => {
  it("checks arguments and normalises a delete by URL", async () => {
    const rpc = { call: vi.fn(async () => ({})) };
    const a = assetsLocals(rpc as never) as Record<string, (...x: unknown[]) => Promise<unknown>>;
    await expect(a.upload("not a blob")).rejects.toMatchObject({ code: "invalid_request" });
    await expect(a.upload(new Blob([]))).rejects.toMatchObject({ code: "invalid_request" });
    await expect(a.upload(new Blob(["x"]), { type: 7 })).rejects.toMatchObject({ code: "invalid_request" });
    await expect(a.upload(new Blob(["x"]), "image/png")).rejects.toMatchObject({ code: "invalid_request" });
    await a.upload(new Blob(["x"], { type: "image/png" }));
    await a.delete("http://7q3k9mzx2b4t.localhost:7480/_blob/01J9ZZZZZZZZZZZZZZZZZZZZZZ");
    await expect(a.delete("")).rejects.toMatchObject({ code: "invalid_request" });
    await expect(a.delete("../../api/x")).rejects.toMatchObject({ code: "invalid_request" });
    await expect(a.delete(7)).rejects.toMatchObject({ code: "invalid_request" });
    expect(rpc.call.mock.calls.map(c => [(c as unknown[])[1], (c as unknown[])[2]])).toEqual([
      ["upload", [{ blob: expect.any(Blob), type: undefined }]],
      ["delete", ["01J9ZZZZZZZZZZZZZZZZZZZZZZ"]],
    ]);
  });
});
