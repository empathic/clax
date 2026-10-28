import { describe, expect, it, vi } from "vitest";
import { SW_SCOPE, SW_URL, registerServiceWorker } from "./pwa";

describe("registerServiceWorker", () => {
  it("registers /sw.js at the root scope", async () => {
    const register = vi.fn(() => Promise.resolve({}));
    registerServiceWorker({ serviceWorker: { register } } as unknown as Navigator);
    expect(register).toHaveBeenCalledWith("/sw.js", { scope: "/" });
    expect([SW_URL, SW_SCOPE]).toEqual(["/sw.js", "/"]);
  });

  it("does nothing without service workers, and ignores a refusal", async () => {
    expect(() => registerServiceWorker({} as Navigator)).not.toThrow();
    const register = vi.fn(() => Promise.reject(new Error("insecure")));
    registerServiceWorker({ serviceWorker: { register } } as unknown as Navigator);
    await Promise.resolve();
    expect(register).toHaveBeenCalledOnce();
  });
});
