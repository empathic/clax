import { describe, expect, it } from "vitest";
import { forgetReporter } from "./main";

describe("Clax.forgetReporter", () => {
  it("needs the instance, queue and user it forgets", async () => {
    for (const r of [{ queue: "q1", user: "u1" }, { instance: "https://clax.example", user: "u1" }, { instance: "https://clax.example", queue: "q1", user: "" }]) {
      await expect(forgetReporter(r as never)).rejects.toThrow(TypeError);
    }
  });

  it("forgets nothing, and fails nothing, where there is no IndexedDB", async () => {
    expect(globalThis.indexedDB).toBeUndefined();
    await expect(forgetReporter({ instance: "https://clax.example/", queue: "q1", user: "u1" })).resolves.toBeUndefined();
  });
});
