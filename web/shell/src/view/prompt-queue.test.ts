import { describe, expect, it } from "vitest";
import { type Ask, promptQueue } from "./prompt-queue";

describe("promptQueue", () => {
  it("shows one prompt at a time, in order", async () => {
    const shown: (Ask | null)[] = [];
    const ask = promptQueue(a => shown.push(a));
    const p = { title: "t", body: "b", allow: "Allow", deny: "Deny" };
    const first = ask(p);
    const second = ask({ ...p, title: "t2" });
    await Promise.resolve();
    expect(shown.filter(Boolean)).toHaveLength(1);
    shown.at(-1)!.answer("allow");
    await expect(first).resolves.toBe("allow");
    await new Promise(r => setTimeout(r, 0));
    expect(shown.at(-1)!.prompt.title).toBe("t2");
    shown.at(-1)!.answer("deny");
    await expect(second).resolves.toBe("deny");
  });
});
